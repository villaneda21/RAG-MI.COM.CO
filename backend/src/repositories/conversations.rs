use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

use crate::domain::models::{
    CaseRecord, ChatMessage, ConversationDetail, ConversationMessage, ConversationSummary,
};
use crate::domain::sessions::{build_summary, is_safe_session_id, new_session_id};
use crate::domain::text::cap_chars;
use crate::domain::timeutil::format_bogota;
use crate::error::AppError;

pub struct NewTurn<'a> {
    pub session_id: &'a str,
    pub question: &'a str,
    pub answer: &'a str,
    pub history: &'a [ChatMessage],
    pub case: Option<&'a CaseRecord>,
}

pub struct ConversationRepository<'a> {
    pool: &'a PgPool,
}

impl<'a> ConversationRepository<'a> {
    pub fn new(pool: &'a PgPool) -> Self {
        Self { pool }
    }

    pub async fn resolve(&self, requested: Option<&str>) -> Result<(String, bool), AppError> {
        if let Some(raw) = requested
            .map(str::trim)
            .filter(|value| is_safe_session_id(value))
        {
            let status = self.status_of(raw).await?;
            if status.as_deref() != Some("closed") {
                return Ok((raw.to_string(), false));
            }
            return Ok((new_session_id(Utc::now()), true));
        }
        Ok((new_session_id(Utc::now()), true))
    }

    pub async fn record_turn(&self, turn: NewTurn<'_>) -> Result<(), AppError> {
        if !is_safe_session_id(turn.session_id) {
            return Err(AppError::BadRequest(
                "session_id inválido para guardar la conversación.".into(),
            ));
        }
        let now = Utc::now();
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(session_lock_key(turn.session_id))
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            r#"
            INSERT INTO conversations (
                id, session_id, created_at, updated_at, status, case_id, correo, categoria, summary
            )
            VALUES ($1, $2, $3, $3, 'open', '', '', '', '')
            ON CONFLICT (session_id) DO NOTHING
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(turn.session_id)
        .bind(now)
        .execute(&mut *tx)
        .await?;

        let row = sqlx::query_as::<_, ConversationRow>(
            r#"
            SELECT id, session_id, created_at, updated_at, status, case_id, correo, categoria, summary
            FROM conversations
            WHERE session_id = $1
            FOR UPDATE
            "#,
        )
        .bind(turn.session_id)
        .fetch_one(&mut *tx)
        .await?;

        let (existing_count,): (i64,) = sqlx::query_as(
            "SELECT COUNT(*)::bigint FROM conversation_messages WHERE conversation_id = $1",
        )
        .bind(row.id)
        .fetch_one(&mut *tx)
        .await?;

        if existing_count == 0 {
            for (offset, message) in turn.history.iter().enumerate() {
                let position = i32::try_from(offset).unwrap_or(i32::MAX);
                insert_message(
                    &mut tx,
                    row.id,
                    position,
                    &message.role,
                    &message.content,
                    now,
                )
                .await?;
            }
        }

        let (max_position,): (i32,) = sqlx::query_as(
            "SELECT COALESCE(MAX(position), -1)::int4 FROM conversation_messages WHERE conversation_id = $1",
        )
        .bind(row.id)
        .fetch_one(&mut *tx)
        .await?;
        let user_position = max_position.saturating_add(1);
        let assistant_position = user_position.saturating_add(1);
        insert_message(&mut tx, row.id, user_position, "user", turn.question, now).await?;
        insert_message(
            &mut tx,
            row.id,
            assistant_position,
            "assistant",
            turn.answer,
            now,
        )
        .await?;

        let messages = sqlx::query_as::<_, MessageRow>(
            r#"
            SELECT role, content
            FROM conversation_messages
            WHERE conversation_id = $1
            ORDER BY position ASC
            "#,
        )
        .bind(row.id)
        .fetch_all(&mut *tx)
        .await?;
        let pairs: Vec<(String, String)> = messages
            .iter()
            .map(|message| (message.role.clone(), message.content.clone()))
            .collect();

        let (status, case_id, correo, categoria) = if let Some(case) = turn.case {
            (
                "closed".to_string(),
                case.case_id.clone(),
                case.correo.clone(),
                case.categoria.clone(),
            )
        } else {
            (row.status, row.case_id, row.correo, row.categoria)
        };
        let summary = build_summary(&categoria, &correo, &pairs);
        sqlx::query(
            r#"
            UPDATE conversations
            SET updated_at = $2,
                status = $3,
                case_id = $4,
                correo = $5,
                categoria = $6,
                summary = $7
            WHERE id = $1
            "#,
        )
        .bind(row.id)
        .bind(now)
        .bind(&status)
        .bind(&case_id)
        .bind(&correo)
        .bind(&categoria)
        .bind(&summary)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        tracing::info!(
            session_id = %turn.session_id,
            status = %status,
            messages = pairs.len(),
            "conversación actualizada"
        );
        Ok(())
    }

    pub async fn list(&self, limit: i64) -> Result<Vec<ConversationSummary>, AppError> {
        let rows = sqlx::query_as::<_, ConversationListRow>(
            r#"
            SELECT c.session_id, c.created_at, c.updated_at, c.status, c.case_id, c.correo,
                   c.categoria, c.summary,
                   (SELECT COUNT(*)::bigint FROM conversation_messages m WHERE m.conversation_id = c.id) AS message_count
            FROM conversations c
            ORDER BY c.updated_at DESC
            LIMIT $1
            "#,
        )
        .bind(limit.clamp(1, 200))
        .fetch_all(self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(ConversationListRow::into_summary)
            .collect())
    }

    pub async fn get(&self, session_id: &str) -> Result<Option<ConversationDetail>, AppError> {
        if !is_safe_session_id(session_id) {
            return Ok(None);
        }
        let Some(row) = sqlx::query_as::<_, ConversationRow>(
            r#"
            SELECT id, session_id, created_at, updated_at, status, case_id, correo, categoria, summary
            FROM conversations
            WHERE session_id = $1
            "#,
        )
        .bind(session_id)
        .fetch_optional(self.pool)
        .await?
        else {
            return Ok(None);
        };
        let messages = sqlx::query_as::<_, StoredMessageRow>(
            r#"
            SELECT role, content, created_at
            FROM conversation_messages
            WHERE conversation_id = $1
            ORDER BY position ASC
            "#,
        )
        .bind(row.id)
        .fetch_all(self.pool)
        .await?;
        let message_count = i64::try_from(messages.len()).unwrap_or(i64::MAX);
        Ok(Some(ConversationDetail {
            session_id: row.session_id,
            created_at: format_bogota(row.created_at),
            updated_at: format_bogota(row.updated_at),
            status: row.status,
            case_id: row.case_id,
            correo: row.correo,
            categoria: row.categoria,
            summary: row.summary,
            message_count,
            messages: messages
                .into_iter()
                .map(|message| ConversationMessage {
                    role: message.role,
                    content: message.content,
                    created_at: format_bogota(message.created_at),
                })
                .collect(),
        }))
    }

    async fn status_of(&self, session_id: &str) -> Result<Option<String>, AppError> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT status FROM conversations WHERE session_id = $1")
                .bind(session_id)
                .fetch_optional(self.pool)
                .await?;
        Ok(row.map(|item| item.0))
    }
}

async fn insert_message(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    conversation_id: Uuid,
    position: i32,
    role: &str,
    content: &str,
    created_at: DateTime<Utc>,
) -> Result<(), AppError> {
    let stored = cap_chars(content.trim(), 16_000);
    if stored.is_empty() {
        return Err(AppError::BadRequest(
            "Un mensaje de la conversación está vacío.".into(),
        ));
    }
    sqlx::query(
        r#"
        INSERT INTO conversation_messages (id, conversation_id, position, role, content, created_at)
        VALUES ($1, $2, $3, $4, $5, $6)
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(conversation_id)
    .bind(position)
    .bind(role)
    .bind(stored)
    .bind(created_at)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn session_lock_key(session_id: &str) -> i64 {
    let digest = Sha256::digest(session_id.as_bytes());
    let mut bytes = [0_u8; 8];
    bytes.copy_from_slice(&digest[..8]);
    i64::from_be_bytes(bytes)
}

#[derive(sqlx::FromRow)]
struct ConversationRow {
    id: Uuid,
    session_id: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    status: String,
    case_id: String,
    correo: String,
    categoria: String,
    summary: String,
}

#[derive(sqlx::FromRow)]
struct ConversationListRow {
    session_id: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    status: String,
    case_id: String,
    correo: String,
    categoria: String,
    summary: String,
    message_count: i64,
}

impl ConversationListRow {
    fn into_summary(self) -> ConversationSummary {
        ConversationSummary {
            session_id: self.session_id,
            created_at: format_bogota(self.created_at),
            updated_at: format_bogota(self.updated_at),
            status: self.status,
            case_id: self.case_id,
            correo: self.correo,
            categoria: self.categoria,
            summary: self.summary,
            message_count: self.message_count,
        }
    }
}

#[derive(sqlx::FromRow)]
struct MessageRow {
    role: String,
    content: String,
}

#[derive(sqlx::FromRow)]
struct StoredMessageRow {
    role: String,
    content: String,
    created_at: DateTime<Utc>,
}
