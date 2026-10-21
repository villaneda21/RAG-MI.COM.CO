//! Persistencia PostgreSQL del centro de ayuda.

use std::path::PathBuf;
use std::str::FromStr;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::config::HELPDESK_DATABASE_URL;

use super::logic::{
    self, agent_notice, handoff_notice, priority_for, reopen_notice, route_department,
    ticket_label, transfer_notice, DEFAULT_CLOSE_MINUTES, DEFAULT_CLOSE_TEMPLATE,
    DEFAULT_WARNING_MINUTES, DEFAULT_WARNING_TEMPLATE, RESOLVED_NOTICE,
};

#[derive(Debug, thiserror::Error)]
pub enum HelpdeskError {
    #[error("{0}")]
    BadRequest(String),
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Conflict(String),
    #[error("{0}")]
    Database(String),
}

#[derive(Clone)]
pub struct Helpdesk {
    pub pool: PgPool,
    pub media_dir: PathBuf,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Customer {
    pub id: Uuid,
    pub name: String,
    pub domain: String,
    pub email_micomco: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Department {
    pub id: Uuid,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Agent {
    pub id: Uuid,
    pub name: String,
    pub email: String,
    pub department_id: Uuid,
    pub department_name: String,
    pub is_admin: bool,
    pub is_active: bool,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct TicketRow {
    pub id: Uuid,
    pub customer_id: Uuid,
    pub assigned_agent_id: Option<Uuid>,
    pub department_id: Uuid,
    pub status: String,
    pub priority: String,
    pub auto_timeout_enabled: bool,
    pub last_client_message_at: DateTime<Utc>,
    pub warning_sent_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub customer_name: String,
    pub customer_domain: String,
    pub email_micomco: String,
    pub department_name: String,
    pub agent_name: Option<String>,
    pub preview: Option<String>,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Message {
    pub id: Uuid,
    pub ticket_id: Uuid,
    pub sender_type: String,
    pub content: String,
    pub media_type: String,
    pub media_url: Option<String>,
    pub is_internal_note: bool,
    pub author_name: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Snippet {
    pub id: Uuid,
    pub shortcut: String,
    pub title: String,
    pub content: String,
    pub department_id: Option<Uuid>,
    pub department_name: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct AuditEntry {
    pub id: Uuid,
    pub ticket_id: Uuid,
    pub agent_id: Option<Uuid>,
    pub action_type: String,
    pub details: String,
    pub created_at: DateTime<Utc>,
    pub agent_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BotSettings {
    pub inactivity_warning_minutes: i32,
    pub inactivity_close_minutes: i32,
    pub warning_message_template: String,
    pub close_message_template: String,
    pub auto_close_enabled: bool,
}

impl Default for BotSettings {
    fn default() -> Self {
        Self {
            inactivity_warning_minutes: DEFAULT_WARNING_MINUTES,
            inactivity_close_minutes: DEFAULT_CLOSE_MINUTES,
            warning_message_template: DEFAULT_WARNING_TEMPLATE.to_string(),
            close_message_template: DEFAULT_CLOSE_TEMPLATE.to_string(),
            auto_close_enabled: true,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct TicketFilter {
    pub department_id: Option<Uuid>,
    pub statuses: Vec<String>,
    pub filter_status: bool,
    pub agent_id: Option<Uuid>,
    pub ticket_id: Option<Uuid>,
}

struct NewMessage<'a> {
    ticket_id: Uuid,
    sender_type: &'a str,
    content: &'a str,
    media_type: &'a str,
    media_url: Option<&'a str>,
    internal: bool,
    author_name: Option<&'a str>,
}

const TICKET_SQL: &str = r#"
SELECT
    t.id,
    t.customer_id,
    t.assigned_agent_id,
    t.department_id,
    t.status,
    t.priority,
    t.auto_timeout_enabled,
    t.last_client_message_at,
    t.warning_sent_at,
    t.created_at,
    t.updated_at,
    c.name AS customer_name,
    c.domain AS customer_domain,
    c.email_micomco,
    d.name AS department_name,
    a.name AS agent_name,
    (
        SELECT left(m.content, 160)
        FROM messages m
        WHERE m.ticket_id = t.id AND m.is_internal_note = FALSE
        ORDER BY m.created_at DESC
        LIMIT 1
    ) AS preview
FROM tickets t
JOIN customers c ON c.id = t.customer_id
JOIN departments d ON d.id = t.department_id
LEFT JOIN agents a ON a.id = t.assigned_agent_id
WHERE ($1::uuid IS NULL OR t.department_id = $1)
  AND ($2::bool = FALSE OR t.status = ANY($3))
  AND ($4::uuid IS NULL OR t.assigned_agent_id = $4)
  AND ($5::uuid IS NULL OR t.id = $5)
ORDER BY t.updated_at DESC
LIMIT 200
"#;

impl Helpdesk {
    pub async fn connect(database_url: &str, media_dir: PathBuf) -> Result<Self, String> {
        Self::connect_with(database_url, media_dir, pool_options()).await
    }

    pub async fn connect_with(
        database_url: &str,
        media_dir: PathBuf,
        options: PgPoolOptions,
    ) -> Result<Self, String> {
        let pool = match open_pool(database_url, options.clone()).await {
            Ok(pool) => pool,
            Err(err) if missing_database(&err) => {
                create_database(database_url).await?;
                open_pool(database_url, options)
                    .await
                    .map_err(|retry| connection_help(database_url, &retry))?
            }
            Err(err) => return Err(connection_help(database_url, &err)),
        };
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .map_err(|err| {
                connection_help(
                    database_url,
                    &format!("no se pudieron aplicar las migraciones: {err}"),
                )
            })?;
        tokio::fs::create_dir_all(&media_dir)
            .await
            .map_err(|err| format!("No se pudo crear el directorio de medios: {err}"))?;
        Ok(Self { pool, media_dir })
    }

    pub async fn open_ticket(
        &self,
        name: &str,
        domain: &str,
        email: &str,
        reason: &str,
    ) -> Result<(Customer, TicketRow), HelpdeskError> {
        let name = require_text(name, "El nombre es obligatorio.", 120)?;
        let domain = require_text(domain, "El dominio es obligatorio.", 160)?;
        let email = require_text(email, "El correo Mi.com.co es obligatorio.", 160)?;
        if !email.contains('@') || !email.contains('.') {
            return Err(HelpdeskError::BadRequest(
                "El correo Mi.com.co no es válido.".into(),
            ));
        }
        let reason = require_text(reason, "Cuéntanos la falla o el motivo.", 4000)?;
        let department_name = route_department(&reason);
        let priority = priority_for(&reason);
        let department = self.department_by_name(department_name).await?;
        let mut tx = self.pool.begin().await.map_err(db_err)?;
        let customer = upsert_customer(&mut tx, &name, &domain, &email).await?;
        let now = Utc::now();
        let ticket_id: Uuid = sqlx::query_scalar(
            r#"
            INSERT INTO tickets (
                customer_id, department_id, status, priority, auto_timeout_enabled,
                last_client_message_at, created_at, updated_at
            )
            VALUES ($1, $2, 'en_bot', $3, TRUE, $4, $4, $4)
            RETURNING id
            "#,
        )
        .bind(customer.id)
        .bind(department.id)
        .bind(priority)
        .bind(now)
        .fetch_one(&mut *tx)
        .await
        .map_err(db_err)?;
        insert_message(
            &mut tx,
            &NewMessage {
                ticket_id,
                sender_type: "client",
                content: &reason,
                media_type: "text",
                media_url: None,
                internal: false,
                author_name: Some(&name),
            },
        )
        .await?;
        insert_audit(
            &mut tx,
            ticket_id,
            None,
            "status_change",
            &format!("Ticket creado en {department_name} y atendido por el asistente."),
        )
        .await?;
        tx.commit().await.map_err(db_err)?;
        let ticket = self.ticket_by_id(ticket_id).await?;
        Ok((customer, ticket))
    }

    pub async fn record_client_message(
        &self,
        ticket_id: Uuid,
        content: &str,
        media_type: &str,
        media_url: Option<&str>,
        author_name: &str,
    ) -> Result<Message, HelpdeskError> {
        let content = content.trim();
        if content.is_empty() && media_url.is_none() {
            return Err(HelpdeskError::BadRequest(
                "Escribe un mensaje, adjunta una imagen o una nota de voz.".into(),
            ));
        }
        if content.chars().count() > 8000 {
            return Err(HelpdeskError::BadRequest(
                "El mensaje es demasiado largo.".into(),
            ));
        }
        let mut tx = self.pool.begin().await.map_err(db_err)?;
        let status: String = sqlx::query_scalar("SELECT status FROM tickets WHERE id = $1")
            .bind(ticket_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(db_err)?
            .ok_or_else(|| HelpdeskError::NotFound("No encontramos ese chat.".into()))?;
        if status == "cerrado" || status == "resuelto" {
            return Err(HelpdeskError::Conflict(
                "Este caso ya está cerrado. Hay que abrir una solicitud nueva.".into(),
            ));
        }
        let now = Utc::now();
        sqlx::query(
            r#"
            UPDATE tickets
            SET last_client_message_at = $2,
                warning_sent_at = NULL,
                updated_at = $2,
                status = CASE WHEN status = 'nuevo' THEN 'en_bot' ELSE status END
            WHERE id = $1
            "#,
        )
        .bind(ticket_id)
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
        let message = insert_message(
            &mut tx,
            &NewMessage {
                ticket_id,
                sender_type: "client",
                content: if content.is_empty() {
                    media_placeholder(media_type)
                } else {
                    content
                },
                media_type,
                media_url,
                internal: false,
                author_name: Some(author_name),
            },
        )
        .await?;
        tx.commit().await.map_err(db_err)?;
        Ok(message)
    }

    pub async fn post_bot_message(
        &self,
        ticket_id: Uuid,
        content: &str,
        next_status: Option<&str>,
        action: &str,
        details: &str,
    ) -> Result<(), HelpdeskError> {
        let mut tx = self.pool.begin().await.map_err(db_err)?;
        let now = Utc::now();
        let updated = sqlx::query(
            r#"
            UPDATE tickets
            SET status = COALESCE($2, status),
                updated_at = $3
            WHERE id = $1
            "#,
        )
        .bind(ticket_id)
        .bind(next_status)
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
        if updated.rows_affected() == 0 {
            return Err(HelpdeskError::NotFound("No encontramos ese chat.".into()));
        }
        insert_message(
            &mut tx,
            &NewMessage {
                ticket_id,
                sender_type: "bot",
                content,
                media_type: "text",
                media_url: None,
                internal: false,
                author_name: Some("Asistente"),
            },
        )
        .await?;
        if matches!(next_status, Some("resuelto" | "cerrado")) {
            insert_message(&mut tx, &system_note(ticket_id, RESOLVED_NOTICE)).await?;
        }
        insert_audit(&mut tx, ticket_id, None, action, details).await?;
        tx.commit().await.map_err(db_err)?;
        Ok(())
    }

    pub async fn open_followup(
        &self,
        previous: &TicketRow,
        content: &str,
        media_type: &str,
        media_url: Option<&str>,
    ) -> Result<Uuid, HelpdeskError> {
        let spoken = if content.trim().is_empty() {
            media_placeholder(media_type)
        } else {
            content.trim()
        };
        if spoken.chars().count() > 8000 {
            return Err(HelpdeskError::BadRequest(
                "El mensaje es demasiado largo.".into(),
            ));
        }
        let department_name = route_department(spoken);
        let department = self.department_by_name(department_name).await?;
        let priority = priority_for(spoken);
        let mut tx = self.pool.begin().await.map_err(db_err)?;
        let now = Utc::now();
        let ticket_id: Uuid = sqlx::query_scalar(
            r#"
            INSERT INTO tickets (
                customer_id, department_id, status, priority, auto_timeout_enabled,
                last_client_message_at, created_at, updated_at
            )
            VALUES ($1, $2, 'en_bot', $3, TRUE, $4, $4, $4)
            RETURNING id
            "#,
        )
        .bind(previous.customer_id)
        .bind(department.id)
        .bind(priority)
        .bind(now)
        .fetch_one(&mut *tx)
        .await
        .map_err(db_err)?;
        let notice = reopen_notice(&ticket_label(ticket_id));
        insert_message(&mut tx, &system_note(ticket_id, &notice)).await?;
        insert_message(
            &mut tx,
            &NewMessage {
                ticket_id,
                sender_type: "client",
                content: spoken,
                media_type,
                media_url,
                internal: false,
                author_name: Some(&previous.customer_name),
            },
        )
        .await?;
        insert_audit(
            &mut tx,
            ticket_id,
            None,
            "status_change",
            &format!(
                "Nueva solicitud {} abierta porque el caso anterior estaba cerrado.",
                ticket_label(ticket_id)
            ),
        )
        .await?;
        tx.commit().await.map_err(db_err)?;
        Ok(ticket_id)
    }

    pub async fn list_tickets(
        &self,
        filter: &TicketFilter,
    ) -> Result<Vec<TicketRow>, HelpdeskError> {
        sqlx::query_as::<_, TicketRow>(TICKET_SQL)
            .bind(filter.department_id)
            .bind(filter.filter_status)
            .bind(&filter.statuses)
            .bind(filter.agent_id)
            .bind(filter.ticket_id)
            .fetch_all(&self.pool)
            .await
            .map_err(db_err)
    }

    pub async fn ticket_by_id(&self, id: Uuid) -> Result<TicketRow, HelpdeskError> {
        let filter = TicketFilter {
            ticket_id: Some(id),
            ..TicketFilter::default()
        };
        self.list_tickets(&filter)
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| HelpdeskError::NotFound("No encontramos ese chat.".into()))
    }

    pub async fn messages(
        &self,
        ticket_id: Uuid,
        public_only: bool,
    ) -> Result<Vec<Message>, HelpdeskError> {
        sqlx::query_as(
            r#"
            SELECT id, ticket_id, sender_type, content, media_type, media_url,
                   is_internal_note, author_name, created_at
            FROM messages
            WHERE ticket_id = $1
              AND ($2::bool = FALSE OR is_internal_note = FALSE)
            ORDER BY created_at ASC
            "#,
        )
        .bind(ticket_id)
        .bind(public_only)
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)
    }

    pub async fn audit_log(&self, ticket_id: Uuid) -> Result<Vec<AuditEntry>, HelpdeskError> {
        sqlx::query_as(
            r#"
            SELECT l.id, l.ticket_id, l.agent_id, l.action_type, l.details, l.created_at,
                   a.name AS agent_name
            FROM ticket_audit_logs l
            LEFT JOIN agents a ON a.id = l.agent_id
            WHERE l.ticket_id = $1
            ORDER BY l.created_at ASC
            "#,
        )
        .bind(ticket_id)
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)
    }

    pub async fn departments(&self) -> Result<Vec<Department>, HelpdeskError> {
        sqlx::query_as("SELECT id, name FROM departments ORDER BY id")
            .fetch_all(&self.pool)
            .await
            .map_err(db_err)
    }

    pub async fn agents(&self) -> Result<Vec<Agent>, HelpdeskError> {
        sqlx::query_as(
            r#"
            SELECT a.id, a.name, a.email, a.department_id, d.name AS department_name,
                   a.is_admin, a.is_active
            FROM agents a
            JOIN departments d ON d.id = a.department_id
            WHERE a.is_active = TRUE
            ORDER BY a.is_admin, a.name
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)
    }

    pub async fn agent_name(&self, id: Uuid) -> Result<String, HelpdeskError> {
        sqlx::query_scalar("SELECT name FROM agents WHERE id = $1 AND is_active = TRUE")
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(db_err)?
            .ok_or_else(|| HelpdeskError::BadRequest("No encontramos ese agente.".into()))
    }

    pub async fn transfer(
        &self,
        ticket_id: Uuid,
        department_id: Uuid,
        agent_id: Option<Uuid>,
        actor_id: Option<Uuid>,
    ) -> Result<TicketRow, HelpdeskError> {
        let current = self.ticket_by_id(ticket_id).await?;
        let department = self.department_by_id(department_id).await?;
        let agent_name = match agent_id {
            Some(id) => Some(self.agent_name(id).await?),
            None => None,
        };
        let status = if agent_id.is_some() {
            "en_proceso"
        } else {
            "esperando_agente"
        };
        let actor_label = match actor_id {
            Some(id) => self.agent_name(id).await?,
            None => "Sistema".to_string(),
        };
        let destination = agent_name
            .clone()
            .unwrap_or_else(|| "sin agente asignado".to_string());
        let origin_agent = current
            .agent_name
            .clone()
            .unwrap_or_else(|| "sin agente".to_string());
        let details = format!(
            "{actor_label} transfirió el chat de {} / {origin_agent} a {} / {destination}.",
            current.department_name, department.name
        );
        let mut tx = self.pool.begin().await.map_err(db_err)?;
        let updated = sqlx::query(
            r#"
            UPDATE tickets
            SET department_id = $2,
                assigned_agent_id = $3,
                status = $4,
                updated_at = NOW()
            WHERE id = $1
            "#,
        )
        .bind(ticket_id)
        .bind(department_id)
        .bind(agent_id)
        .bind(status)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
        if updated.rows_affected() == 0 {
            return Err(HelpdeskError::NotFound("No encontramos ese chat.".into()));
        }
        insert_audit(&mut tx, ticket_id, actor_id, "transfer", &details).await?;
        if current.department_id != department_id {
            let notice = transfer_notice(&department.name);
            insert_message(&mut tx, &system_note(ticket_id, &notice)).await?;
        }
        if agent_id.is_some() && agent_id != current.assigned_agent_id {
            if let Some(name) = agent_name.as_deref() {
                insert_assumption(&mut tx, ticket_id, &agent_notice(name)).await?;
            }
        }
        tx.commit().await.map_err(db_err)?;
        self.ticket_by_id(ticket_id).await
    }

    pub async fn handoff_to_team(&self, ticket_id: Uuid, area: &str) -> Result<(), HelpdeskError> {
        let notice = handoff_notice(&ticket_label(ticket_id), area);
        self.handoff_with_notice(ticket_id, area, &notice).await
    }

    pub async fn handoff_with_notice(
        &self,
        ticket_id: Uuid,
        area: &str,
        notice: &str,
    ) -> Result<(), HelpdeskError> {
        let department = self.department_by_name(area).await?;
        let mut tx = self.pool.begin().await.map_err(db_err)?;
        let updated = sqlx::query(
            r#"
            UPDATE tickets
            SET department_id = $2,
                assigned_agent_id = NULL,
                status = 'esperando_agente',
                updated_at = NOW()
            WHERE id = $1
            "#,
        )
        .bind(ticket_id)
        .bind(department.id)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
        if updated.rows_affected() == 0 {
            return Err(HelpdeskError::NotFound("No encontramos ese chat.".into()));
        }
        insert_message(&mut tx, &system_note(ticket_id, notice)).await?;
        insert_audit(
            &mut tx,
            ticket_id,
            None,
            "status_change",
            &format!("Escalado al equipo de {area}."),
        )
        .await?;
        tx.commit().await.map_err(db_err)?;
        Ok(())
    }

    pub async fn set_timeout(
        &self,
        ticket_id: Uuid,
        enabled: bool,
        actor_id: Option<Uuid>,
    ) -> Result<TicketRow, HelpdeskError> {
        let mut tx = self.pool.begin().await.map_err(db_err)?;
        let updated = sqlx::query(
            "UPDATE tickets SET auto_timeout_enabled = $2, updated_at = NOW() WHERE id = $1",
        )
        .bind(ticket_id)
        .bind(enabled)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
        if updated.rows_affected() == 0 {
            return Err(HelpdeskError::NotFound("No encontramos ese chat.".into()));
        }
        let state = if enabled { "activado" } else { "pausado" };
        insert_audit(
            &mut tx,
            ticket_id,
            actor_id,
            "auto_timeout",
            &format!("Temporizador de inactividad {state}."),
        )
        .await?;
        tx.commit().await.map_err(db_err)?;
        self.ticket_by_id(ticket_id).await
    }

    pub async fn change_status(
        &self,
        ticket_id: Uuid,
        status: &str,
        actor_id: Option<Uuid>,
    ) -> Result<TicketRow, HelpdeskError> {
        if !matches!(
            status,
            "nuevo" | "en_bot" | "esperando_agente" | "en_proceso" | "resuelto" | "cerrado"
        ) {
            return Err(HelpdeskError::BadRequest("Ese estado no existe.".into()));
        }
        let current = self.ticket_by_id(ticket_id).await?;
        if current.status == status {
            return Ok(current);
        }
        let mut tx = self.pool.begin().await.map_err(db_err)?;
        sqlx::query("UPDATE tickets SET status = $2, updated_at = NOW() WHERE id = $1")
            .bind(ticket_id)
            .bind(status)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        insert_audit(
            &mut tx,
            ticket_id,
            actor_id,
            "status_change",
            &format!("Estado cambiado de {} a {status}.", current.status),
        )
        .await?;
        if matches!(status, "resuelto" | "cerrado") {
            insert_message(&mut tx, &system_note(ticket_id, RESOLVED_NOTICE)).await?;
        }
        tx.commit().await.map_err(db_err)?;
        self.ticket_by_id(ticket_id).await
    }

    pub async fn add_agent_message(
        &self,
        ticket_id: Uuid,
        agent_id: Uuid,
        content: &str,
        internal: bool,
    ) -> Result<Message, HelpdeskError> {
        let content = require_text(content, "La nota está vacía.", 8000)?;
        let author = self.agent_name(agent_id).await?;
        let current = self.ticket_by_id(ticket_id).await?;
        let mut tx = self.pool.begin().await.map_err(db_err)?;
        let message = insert_message(
            &mut tx,
            &NewMessage {
                ticket_id,
                sender_type: "agent",
                content: &content,
                media_type: "text",
                media_url: None,
                internal,
                author_name: Some(&author),
            },
        )
        .await?;
        if internal {
            insert_audit(
                &mut tx,
                ticket_id,
                Some(agent_id),
                "note_added",
                &format!("{author} agregó una nota interna."),
            )
            .await?;
            sqlx::query("UPDATE tickets SET updated_at = NOW() WHERE id = $1")
                .bind(ticket_id)
                .execute(&mut *tx)
                .await
                .map_err(db_err)?;
        } else {
            let next = if matches!(
                current.status.as_str(),
                "nuevo" | "en_bot" | "esperando_agente" | "resuelto"
            ) {
                "en_proceso"
            } else {
                current.status.as_str()
            };
            sqlx::query(
                r#"
                UPDATE tickets
                SET status = $2,
                    assigned_agent_id = COALESCE(assigned_agent_id, $3),
                    updated_at = NOW()
                WHERE id = $1
                "#,
            )
            .bind(ticket_id)
            .bind(next)
            .bind(agent_id)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
            if next != current.status {
                insert_audit(
                    &mut tx,
                    ticket_id,
                    Some(agent_id),
                    "status_change",
                    &format!("{author} tomó el chat. Estado: {next}."),
                )
                .await?;
            }
            if current.assigned_agent_id != Some(agent_id) {
                insert_assumption(&mut tx, ticket_id, &agent_notice(&author)).await?;
            }
        }
        tx.commit().await.map_err(db_err)?;
        Ok(message)
    }

    pub async fn customer_by_id(&self, id: Uuid) -> Result<Customer, HelpdeskError> {
        sqlx::query_as("SELECT id, name, domain, email_micomco, created_at, updated_at FROM customers WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(db_err)?
            .ok_or_else(|| HelpdeskError::NotFound("No encontramos ese cliente.".into()))
    }

    pub async fn customers(&self) -> Result<Vec<Customer>, HelpdeskError> {
        sqlx::query_as(
            "SELECT id, name, domain, email_micomco, created_at, updated_at FROM customers ORDER BY updated_at DESC LIMIT 100",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)
    }

    pub async fn update_customer(
        &self,
        id: Uuid,
        name: &str,
        domain: &str,
        email: &str,
    ) -> Result<Customer, HelpdeskError> {
        let name = require_text(name, "El nombre es obligatorio.", 120)?;
        let domain = require_text(domain, "El dominio es obligatorio.", 160)?;
        let email = require_text(email, "El correo es obligatorio.", 160)?;
        if !email.contains('@') {
            return Err(HelpdeskError::BadRequest("El correo no es válido.".into()));
        }
        sqlx::query_as(
            r#"
            UPDATE customers
            SET name = $2, domain = $3, email_micomco = $4, updated_at = NOW()
            WHERE id = $1
            RETURNING id, name, domain, email_micomco, created_at, updated_at
            "#,
        )
        .bind(id)
        .bind(name)
        .bind(domain)
        .bind(email)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?
        .ok_or_else(|| HelpdeskError::NotFound("No encontramos ese cliente.".into()))
    }

    pub async fn snippets(&self) -> Result<Vec<Snippet>, HelpdeskError> {
        sqlx::query_as(
            r#"
            SELECT s.id, s.shortcut, s.title, s.content, s.department_id,
                   d.name AS department_name, s.created_at
            FROM canned_responses s
            LEFT JOIN departments d ON d.id = s.department_id
            ORDER BY s.shortcut
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)
    }

    pub async fn create_snippet(
        &self,
        shortcut: &str,
        title: &str,
        content: &str,
        department_id: Option<Uuid>,
    ) -> Result<Snippet, HelpdeskError> {
        let shortcut = logic::normalize_shortcut(shortcut)
            .ok_or_else(|| HelpdeskError::BadRequest("El atajo debe ser /palabra.".into()))?;
        let title = require_text(title, "El título es obligatorio.", 120)?;
        let content = require_text(content, "El contenido es obligatorio.", 4000)?;
        if let Some(id) = department_id {
            self.department_by_id(id).await?;
        }
        let id: Uuid = sqlx::query_scalar(
            r#"
            INSERT INTO canned_responses (shortcut, title, content, department_id)
            VALUES ($1, $2, $3, $4)
            RETURNING id
            "#,
        )
        .bind(&shortcut)
        .bind(&title)
        .bind(&content)
        .bind(department_id)
        .fetch_one(&self.pool)
        .await
        .map_err(map_unique)?;
        self.snippet_by_id(id).await
    }

    pub async fn update_snippet(
        &self,
        id: Uuid,
        shortcut: &str,
        title: &str,
        content: &str,
        department_id: Option<Uuid>,
    ) -> Result<Snippet, HelpdeskError> {
        let shortcut = logic::normalize_shortcut(shortcut)
            .ok_or_else(|| HelpdeskError::BadRequest("El atajo debe ser /palabra.".into()))?;
        let title = require_text(title, "El título es obligatorio.", 120)?;
        let content = require_text(content, "El contenido es obligatorio.", 4000)?;
        if let Some(department) = department_id {
            self.department_by_id(department).await?;
        }
        let updated = sqlx::query(
            r#"
            UPDATE canned_responses
            SET shortcut = $2, title = $3, content = $4, department_id = $5
            WHERE id = $1
            "#,
        )
        .bind(id)
        .bind(&shortcut)
        .bind(&title)
        .bind(&content)
        .bind(department_id)
        .execute(&self.pool)
        .await
        .map_err(map_unique)?;
        if updated.rows_affected() == 0 {
            return Err(HelpdeskError::NotFound(
                "No encontramos esa respuesta rápida.".into(),
            ));
        }
        self.snippet_by_id(id).await
    }

    pub async fn delete_snippet(&self, id: Uuid) -> Result<(), HelpdeskError> {
        let deleted = sqlx::query("DELETE FROM canned_responses WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(db_err)?;
        if deleted.rows_affected() == 0 {
            return Err(HelpdeskError::NotFound(
                "No encontramos esa respuesta rápida.".into(),
            ));
        }
        Ok(())
    }

    pub async fn rules(&self) -> Result<BotSettings, HelpdeskError> {
        let rows: Vec<(String, Value)> =
            sqlx::query_as("SELECT rule_key, rule_value FROM bot_rules")
                .fetch_all(&self.pool)
                .await
                .map_err(db_err)?;
        let mut rules = BotSettings::default();
        for (key, value) in rows {
            match key.as_str() {
                "inactivity_warning_minutes" => {
                    rules.inactivity_warning_minutes =
                        json_i32(&value, rules.inactivity_warning_minutes);
                }
                "inactivity_close_minutes" => {
                    rules.inactivity_close_minutes =
                        json_i32(&value, rules.inactivity_close_minutes);
                }
                "warning_message_template" => {
                    rules.warning_message_template =
                        json_text(&value, &rules.warning_message_template);
                }
                "close_message_template" => {
                    rules.close_message_template = json_text(&value, &rules.close_message_template);
                }
                "auto_close_enabled" => {
                    rules.auto_close_enabled = json_bool(&value, rules.auto_close_enabled);
                }
                _ => {}
            }
        }
        Ok(rules)
    }

    pub async fn save_rules(&self, rules: &BotSettings) -> Result<BotSettings, HelpdeskError> {
        let pairs = [
            (
                "inactivity_warning_minutes",
                json!(rules.inactivity_warning_minutes.clamp(0, 10_080)),
            ),
            (
                "inactivity_close_minutes",
                json!(rules.inactivity_close_minutes.clamp(0, 10_080)),
            ),
            (
                "warning_message_template",
                json!(rules.warning_message_template.trim()),
            ),
            (
                "close_message_template",
                json!(rules.close_message_template.trim()),
            ),
            ("auto_close_enabled", json!(rules.auto_close_enabled)),
        ];
        if rules.warning_message_template.trim().is_empty()
            || rules.close_message_template.trim().is_empty()
        {
            return Err(HelpdeskError::BadRequest(
                "Las plantillas de advertencia y cierre no pueden quedar vacías.".into(),
            ));
        }
        for (key, value) in pairs {
            sqlx::query(
                r#"
                INSERT INTO bot_rules (rule_key, rule_value, description, updated_at)
                VALUES ($1, $2, '', NOW())
                ON CONFLICT (rule_key) DO UPDATE
                SET rule_value = EXCLUDED.rule_value, updated_at = NOW()
                "#,
            )
            .bind(key)
            .bind(&value)
            .execute(&self.pool)
            .await
            .map_err(db_err)?;
        }
        self.rules().await
    }

    pub async fn tickets_needing_warning(&self, minutes: i32) -> Result<Vec<Uuid>, HelpdeskError> {
        let statuses = timeout_statuses();
        sqlx::query_scalar(
            r#"
            SELECT id FROM tickets
            WHERE status = ANY($1)
              AND auto_timeout_enabled = TRUE
              AND warning_sent_at IS NULL
              AND last_client_message_at <= NOW() - ($2::int * INTERVAL '1 minute')
            "#,
        )
        .bind(&statuses)
        .bind(minutes)
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)
    }

    pub async fn tickets_needing_close(&self, minutes: i32) -> Result<Vec<Uuid>, HelpdeskError> {
        let statuses = timeout_statuses();
        sqlx::query_scalar(
            r#"
            SELECT id FROM tickets
            WHERE status = ANY($1)
              AND auto_timeout_enabled = TRUE
              AND warning_sent_at IS NOT NULL
              AND last_client_message_at <= warning_sent_at
              AND warning_sent_at <= NOW() - ($2::int * INTERVAL '1 minute')
            "#,
        )
        .bind(&statuses)
        .bind(minutes)
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)
    }

    pub async fn mark_warning(
        &self,
        ticket_id: Uuid,
        template: &str,
    ) -> Result<bool, HelpdeskError> {
        let statuses = timeout_statuses();
        let mut tx = self.pool.begin().await.map_err(db_err)?;
        let marked: Option<Uuid> = sqlx::query_scalar(
            r#"
            UPDATE tickets
            SET warning_sent_at = NOW(), updated_at = NOW()
            WHERE id = $1
              AND warning_sent_at IS NULL
              AND auto_timeout_enabled = TRUE
              AND status = ANY($2)
            RETURNING id
            "#,
        )
        .bind(ticket_id)
        .bind(&statuses)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db_err)?;
        if marked.is_none() {
            return Ok(false);
        }
        insert_message(
            &mut tx,
            &NewMessage {
                ticket_id,
                sender_type: "bot",
                content: template,
                media_type: "text",
                media_url: None,
                internal: false,
                author_name: Some("Asistente"),
            },
        )
        .await?;
        insert_audit(
            &mut tx,
            ticket_id,
            None,
            "auto_timeout",
            "Advertencia de inactividad enviada.",
        )
        .await?;
        tx.commit().await.map_err(db_err)?;
        Ok(true)
    }

    pub async fn mark_closed_for_inactivity(
        &self,
        ticket_id: Uuid,
        template: &str,
        close_minutes: i32,
    ) -> Result<bool, HelpdeskError> {
        let statuses = timeout_statuses();
        let mut tx = self.pool.begin().await.map_err(db_err)?;
        let marked: Option<Uuid> = sqlx::query_scalar(
            r#"
            UPDATE tickets
            SET status = 'cerrado', updated_at = NOW()
            WHERE id = $1
              AND auto_timeout_enabled = TRUE
              AND warning_sent_at IS NOT NULL
              AND last_client_message_at <= warning_sent_at
              AND warning_sent_at <= NOW() - ($2::int * INTERVAL '1 minute')
              AND status = ANY($3)
            RETURNING id
            "#,
        )
        .bind(ticket_id)
        .bind(close_minutes)
        .bind(&statuses)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db_err)?;
        if marked.is_none() {
            return Ok(false);
        }
        insert_message(
            &mut tx,
            &NewMessage {
                ticket_id,
                sender_type: "bot",
                content: template,
                media_type: "text",
                media_url: None,
                internal: false,
                author_name: Some("Asistente"),
            },
        )
        .await?;
        insert_message(&mut tx, &system_note(ticket_id, RESOLVED_NOTICE)).await?;
        insert_audit(
            &mut tx,
            ticket_id,
            None,
            "auto_timeout",
            "Cierre automático por inactividad.",
        )
        .await?;
        tx.commit().await.map_err(db_err)?;
        Ok(true)
    }

    async fn department_by_name(&self, name: &str) -> Result<Department, HelpdeskError> {
        sqlx::query_as("SELECT id, name FROM departments WHERE name = $1")
            .bind(name)
            .fetch_optional(&self.pool)
            .await
            .map_err(db_err)?
            .ok_or_else(|| HelpdeskError::Database(format!("Falta el departamento {name}.")))
    }

    async fn department_by_id(&self, id: Uuid) -> Result<Department, HelpdeskError> {
        sqlx::query_as("SELECT id, name FROM departments WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(db_err)?
            .ok_or_else(|| HelpdeskError::BadRequest("No encontramos ese departamento.".into()))
    }

    async fn snippet_by_id(&self, id: Uuid) -> Result<Snippet, HelpdeskError> {
        sqlx::query_as(
            r#"
            SELECT s.id, s.shortcut, s.title, s.content, s.department_id,
                   d.name AS department_name, s.created_at
            FROM canned_responses s
            LEFT JOIN departments d ON d.id = s.department_id
            WHERE s.id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?
        .ok_or_else(|| HelpdeskError::NotFound("No encontramos esa respuesta rápida.".into()))
    }
}

fn timeout_statuses() -> Vec<String> {
    logic::TIMEOUT_STATUSES
        .iter()
        .map(|status| (*status).to_string())
        .collect()
}

fn media_placeholder(media_type: &str) -> &'static str {
    match media_type {
        "audio" => "Nota de voz",
        "image" => "Imagen",
        _ => "Archivo",
    }
}

fn message_stamp() -> DateTime<Utc> {
    use std::sync::atomic::{AtomicI64, Ordering};
    static LAST: AtomicI64 = AtomicI64::new(0);
    let now = Utc::now().timestamp_micros();
    let mut previous = LAST.load(Ordering::Relaxed);
    loop {
        let next = now.max(previous.saturating_add(1));
        match LAST.compare_exchange_weak(previous, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return DateTime::from_timestamp_micros(next).unwrap_or_else(Utc::now),
            Err(seen) => previous = seen,
        }
    }
}

async fn insert_assumption(
    tx: &mut Transaction<'_, Postgres>,
    ticket_id: Uuid,
    content: &str,
) -> Result<(), HelpdeskError> {
    let previous: Option<(Uuid, String, DateTime<Utc>)> = sqlx::query_as(
        r#"
        SELECT id, content, created_at
        FROM messages
        WHERE ticket_id = $1
          AND sender_type = 'system'
          AND content LIKE '%ha asumido%'
        ORDER BY created_at DESC
        LIMIT 1
        "#,
    )
    .bind(ticket_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(db_err)?;
    if let Some((id, prior, when)) = previous {
        let age = Utc::now().signed_duration_since(when).num_seconds();
        if prior == content {
            return Ok(());
        }
        if (-5..45).contains(&age) {
            sqlx::query("DELETE FROM messages WHERE id = $1")
                .bind(id)
                .execute(&mut **tx)
                .await
                .map_err(db_err)?;
        }
    }
    insert_message(tx, &system_note(ticket_id, content)).await?;
    Ok(())
}

fn system_note(ticket_id: Uuid, content: &str) -> NewMessage<'_> {
    NewMessage {
        ticket_id,
        sender_type: "system",
        content,
        media_type: "text",
        media_url: None,
        internal: false,
        author_name: Some("Sistema"),
    }
}

fn require_text(value: &str, message: &str, max_chars: usize) -> Result<String, HelpdeskError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(HelpdeskError::BadRequest(message.into()));
    }
    if trimmed.chars().count() > max_chars {
        return Err(HelpdeskError::BadRequest(
            "Uno de los campos es demasiado largo.".into(),
        ));
    }
    Ok(trimmed.to_string())
}

async fn upsert_customer(
    tx: &mut Transaction<'_, Postgres>,
    name: &str,
    domain: &str,
    email: &str,
) -> Result<Customer, HelpdeskError> {
    let existing: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM customers WHERE lower(email_micomco) = lower($1) ORDER BY updated_at DESC LIMIT 1",
    )
    .bind(email)
    .fetch_optional(&mut **tx)
    .await
    .map_err(db_err)?;
    if let Some(id) = existing {
        return sqlx::query_as(
            r#"
            UPDATE customers
            SET name = $2, domain = $3, email_micomco = $4, updated_at = NOW()
            WHERE id = $1
            RETURNING id, name, domain, email_micomco, created_at, updated_at
            "#,
        )
        .bind(id)
        .bind(name)
        .bind(domain)
        .bind(email)
        .fetch_one(&mut **tx)
        .await
        .map_err(db_err);
    }
    sqlx::query_as(
        r#"
        INSERT INTO customers (name, domain, email_micomco)
        VALUES ($1, $2, $3)
        RETURNING id, name, domain, email_micomco, created_at, updated_at
        "#,
    )
    .bind(name)
    .bind(domain)
    .bind(email)
    .fetch_one(&mut **tx)
    .await
    .map_err(db_err)
}

async fn insert_message(
    tx: &mut Transaction<'_, Postgres>,
    message: &NewMessage<'_>,
) -> Result<Message, HelpdeskError> {
    sqlx::query_as(
        r#"
        INSERT INTO messages (
            ticket_id, sender_type, content, media_type, media_url, is_internal_note, author_name,
            created_at
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        RETURNING id, ticket_id, sender_type, content, media_type, media_url,
                  is_internal_note, author_name, created_at
        "#,
    )
    .bind(message.ticket_id)
    .bind(message.sender_type)
    .bind(message.content)
    .bind(message.media_type)
    .bind(message.media_url)
    .bind(message.internal)
    .bind(message.author_name)
    .bind(message_stamp())
    .fetch_one(&mut **tx)
    .await
    .map_err(db_err)
}

async fn insert_audit(
    tx: &mut Transaction<'_, Postgres>,
    ticket_id: Uuid,
    agent_id: Option<Uuid>,
    action: &str,
    details: &str,
) -> Result<(), HelpdeskError> {
    sqlx::query(
        r#"
        INSERT INTO ticket_audit_logs (ticket_id, agent_id, action_type, details)
        VALUES ($1, $2, $3, $4)
        "#,
    )
    .bind(ticket_id)
    .bind(agent_id)
    .bind(action)
    .bind(details)
    .execute(&mut **tx)
    .await
    .map_err(db_err)?;
    Ok(())
}

pub fn pool_options() -> PgPoolOptions {
    PgPoolOptions::new()
        .max_connections(20)
        .acquire_timeout(Duration::from_secs(5))
}

async fn open_pool(database_url: &str, options: PgPoolOptions) -> Result<PgPool, String> {
    options
        .connect(database_url)
        .await
        .map_err(|err| err.to_string())
}

fn missing_database(err: &str) -> bool {
    let lowered = err.to_lowercase();
    lowered.contains("does not exist") && lowered.contains("database")
}

async fn create_database(database_url: &str) -> Result<(), String> {
    let options = PgConnectOptions::from_str(database_url)
        .map_err(|err| connection_help(database_url, &err.to_string()))?;
    let name = options.get_database().unwrap_or("rag_micomco").to_string();
    if name.is_empty()
        || !name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
    {
        return Err(connection_help(
            database_url,
            "el nombre de la base no se puede crear solo",
        ));
    }
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(Duration::from_secs(5))
        .connect_with(options.database("postgres"))
        .await
        .map_err(|err| connection_help(database_url, &err.to_string()))?;
    sqlx::query(&format!("CREATE DATABASE {name}"))
        .execute(&admin)
        .await
        .map_err(|err| connection_help(database_url, &err.to_string()))?;
    tracing::info!("Se creó la base PostgreSQL {name}.");
    Ok(())
}

pub fn connection_help(database_url: &str, reason: &str) -> String {
    let target = describe_database_target(database_url);
    format!(
        "No hay conexión con PostgreSQL ({target}). {reason}. \
El puerto 5432 es el PostgreSQL de Windows. El contenedor Docker usa el 5433. \
En .env deja DATABASE_URL={HELPDESK_DATABASE_URL}. \
Luego ejecuta docker compose up -d y recarga esta página."
    )
}

fn describe_database_target(database_url: &str) -> String {
    let rest = database_url
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(database_url);
    let (authority, after_user) = rest.split_once('@').unwrap_or(("", rest));
    let user = authority.split(':').next().filter(|user| !user.is_empty());
    let without_query = after_user.split('?').next().unwrap_or(after_user);
    let (host, database) = without_query
        .split_once('/')
        .unwrap_or((without_query, "rag_micomco"));
    match user {
        Some(user) => format!("usuario {user}, {host}, base {database}"),
        None => format!("{host}, base {database}"),
    }
}

fn db_err(err: sqlx::Error) -> HelpdeskError {
    tracing::error!("PostgreSQL: {err}");
    HelpdeskError::Database("No se pudo guardar la información del centro de ayuda.".into())
}

fn map_unique(err: sqlx::Error) -> HelpdeskError {
    if let sqlx::Error::Database(db) = &err {
        if db.code().as_deref() == Some("23505") {
            return HelpdeskError::BadRequest("Ese atajo ya existe.".into());
        }
    }
    db_err(err)
}

fn json_i32(value: &Value, default: i32) -> i32 {
    value
        .as_i64()
        .map(|number| i32::try_from(number).unwrap_or(default))
        .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
        .unwrap_or(default)
        .clamp(0, 10_080)
}

fn json_bool(value: &Value, default: bool) -> bool {
    if let Some(flag) = value.as_bool() {
        return flag;
    }
    match value.as_str() {
        Some("true" | "1" | "yes" | "si" | "sí") => true,
        Some("false" | "0" | "no") => false,
        _ => default,
    }
}

fn json_text(value: &Value, default: &str) -> String {
    value
        .as_str()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(ToString::to_string)
        .unwrap_or_else(|| default.to_string())
}
