//! Rutas del simulador de cliente, el CRM y la administración.

use std::path::PathBuf;

use axum::body::Bytes;
use axum::extract::{FromRequest, Multipart, Path, Query, Request, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use base64::Engine;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::error::AppError;
use crate::state::AppState;

use super::logic::{media_decision, MediaDecision, GIF_REJECTION};
use super::service::{self, ClientView};
use super::store::{BotSettings, Helpdesk, HelpdeskError, TicketFilter};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/client", get(client_page))
        .route("/crm", get(crm_page))
        .route("/crm/admin", get(crm_page))
        .route("/api/client/triage", post(client_triage))
        .route("/api/client/message", post(client_message))
        .route("/api/client/tickets/:id", get(client_ticket))
        .route("/api/crm/tickets", get(crm_tickets))
        .route("/api/crm/tickets/:id", get(crm_ticket))
        .route("/api/crm/tickets/:id/transfer", put(crm_transfer))
        .route("/api/crm/tickets/:id/timeout-toggle", put(crm_timeout))
        .route("/api/crm/tickets/:id/notes", post(crm_note))
        .route("/api/crm/tickets/:id/reply", post(crm_reply))
        .route("/api/crm/tickets/:id/status", put(crm_status))
        .route(
            "/api/crm/customers",
            get(crm_customers).put(crm_update_customer),
        )
        .route("/api/crm/departments", get(crm_departments))
        .route("/api/crm/agents", get(crm_agents))
        .route(
            "/api/admin/snippets",
            get(list_snippets)
                .post(create_snippet)
                .put(update_snippet_body)
                .delete(delete_snippet_query),
        )
        .route(
            "/api/admin/snippets/:id",
            put(update_snippet_path).delete(delete_snippet_path),
        )
        .route("/api/admin/rules", get(get_rules).put(put_rules))
}

async fn client_page(State(state): State<AppState>) -> Result<Html<String>, AppError> {
    read_page(&state, "client.html").await
}

async fn crm_page(State(state): State<AppState>) -> Result<Html<String>, AppError> {
    read_page(&state, "crm.html").await
}

async fn client_triage(
    State(state): State<AppState>,
    Body(body): Body<TriageBody>,
) -> Result<Json<Value>, AppError> {
    let helpdesk = require(&state).await?;
    let view = service::triage(
        &helpdesk,
        &state.rag,
        &body.name,
        &body.domain,
        &body.email_micomco,
        &body.reason,
    )
    .await
    .map_err(map_helpdesk)?;
    let customer = helpdesk
        .customer_by_id(view.ticket.customer_id)
        .await
        .map_err(map_helpdesk)?;
    Ok(Json(json!({
        "customer": customer,
        "ticket": view.ticket,
        "messages": view.messages,
    })))
}

async fn client_message(
    State(state): State<AppState>,
    request: Request,
) -> Result<Json<Value>, AppError> {
    let helpdesk = require(&state).await?;
    let content_type = request
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    let incoming = if content_type.starts_with("multipart/") {
        read_multipart(request).await?
    } else {
        let bytes = axum::body::to_bytes(request.into_body(), MAX_BODY)
            .await
            .map_err(|_| AppError::unprocessable())?;
        let body: ClientMessageBody =
            serde_json::from_slice(&bytes).map_err(|_| AppError::unprocessable())?;
        incoming_from_json(body)?
    };
    let (media_url, media_type) = match incoming.attachment {
        Some(file) => (
            Some(write_media(&helpdesk, &file.bytes, file.extension).await?),
            file.kind,
        ),
        None => (None, "text"),
    };
    let view = service::accept_client_message(
        &helpdesk,
        &state.rag,
        incoming.ticket_id,
        &incoming.content,
        media_url.as_deref(),
        media_type,
    )
    .await
    .map_err(map_helpdesk)?;
    Ok(view_json(view))
}

async fn client_ticket(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, AppError> {
    let helpdesk = require(&state).await?;
    let view = service::client_view(&helpdesk, id)
        .await
        .map_err(map_helpdesk)?;
    Ok(view_json(view))
}

async fn crm_tickets(
    State(state): State<AppState>,
    Query(query): Query<TicketQuery>,
) -> Result<Json<Value>, AppError> {
    let helpdesk = require(&state).await?;
    let filter = ticket_filter(query)?;
    let tickets = helpdesk.list_tickets(&filter).await.map_err(map_helpdesk)?;
    Ok(Json(json!({ "tickets": tickets })))
}

async fn crm_ticket(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, AppError> {
    let helpdesk = require(&state).await?;
    let ticket = helpdesk.ticket_by_id(id).await.map_err(map_helpdesk)?;
    let messages = helpdesk.messages(id, false).await.map_err(map_helpdesk)?;
    let audit = helpdesk.audit_log(id).await.map_err(map_helpdesk)?;
    Ok(Json(json!({
        "ticket": ticket,
        "messages": messages,
        "audit": audit,
    })))
}

async fn crm_transfer(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Body(body): Body<TransferBody>,
) -> Result<Json<Value>, AppError> {
    let helpdesk = require(&state).await?;
    let ticket = helpdesk
        .transfer(id, body.department_id, body.agent_id, body.actor_agent_id)
        .await
        .map_err(map_helpdesk)?;
    Ok(Json(json!({ "ticket": ticket })))
}

async fn crm_timeout(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Body(body): Body<TimeoutBody>,
) -> Result<Json<Value>, AppError> {
    let helpdesk = require(&state).await?;
    let enabled = body
        .auto_timeout_enabled
        .or(body.enabled)
        .ok_or_else(|| AppError::bad_request("Indica si el temporizador queda activo."))?;
    let ticket = helpdesk
        .set_timeout(id, enabled, body.agent_id)
        .await
        .map_err(map_helpdesk)?;
    Ok(Json(json!({ "ticket": ticket })))
}

async fn crm_note(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Body(body): Body<AgentTextBody>,
) -> Result<Json<Value>, AppError> {
    let helpdesk = require(&state).await?;
    let message = helpdesk
        .add_agent_message(id, body.agent_id, &body.content, true)
        .await
        .map_err(map_helpdesk)?;
    Ok(Json(json!({ "message": message })))
}

async fn crm_reply(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Body(body): Body<AgentTextBody>,
) -> Result<Json<Value>, AppError> {
    let helpdesk = require(&state).await?;
    let message = helpdesk
        .add_agent_message(id, body.agent_id, &body.content, false)
        .await
        .map_err(map_helpdesk)?;
    let ticket = helpdesk.ticket_by_id(id).await.map_err(map_helpdesk)?;
    Ok(Json(json!({ "message": message, "ticket": ticket })))
}

async fn crm_status(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Body(body): Body<StatusBody>,
) -> Result<Json<Value>, AppError> {
    let helpdesk = require(&state).await?;
    let ticket = helpdesk
        .change_status(id, &body.status, body.agent_id)
        .await
        .map_err(map_helpdesk)?;
    Ok(Json(json!({ "ticket": ticket })))
}

async fn crm_customers(
    State(state): State<AppState>,
    Query(query): Query<CustomerQuery>,
) -> Result<Json<Value>, AppError> {
    let helpdesk = require(&state).await?;
    if let Some(id) = query.id {
        let customer = helpdesk.customer_by_id(id).await.map_err(map_helpdesk)?;
        return Ok(Json(json!(customer)));
    }
    let customers = helpdesk.customers().await.map_err(map_helpdesk)?;
    Ok(Json(json!({ "customers": customers })))
}

async fn crm_update_customer(
    State(state): State<AppState>,
    Body(body): Body<CustomerBody>,
) -> Result<Json<Value>, AppError> {
    let helpdesk = require(&state).await?;
    let customer = helpdesk
        .update_customer(body.id, &body.name, &body.domain, &body.email_micomco)
        .await
        .map_err(map_helpdesk)?;
    Ok(Json(json!(customer)))
}

async fn crm_departments(State(state): State<AppState>) -> Result<Json<Value>, AppError> {
    let helpdesk = require(&state).await?;
    let departments = helpdesk.departments().await.map_err(map_helpdesk)?;
    Ok(Json(json!({ "departments": departments })))
}

async fn crm_agents(State(state): State<AppState>) -> Result<Json<Value>, AppError> {
    let helpdesk = require(&state).await?;
    let agents = helpdesk.agents().await.map_err(map_helpdesk)?;
    Ok(Json(json!({ "agents": agents })))
}

async fn list_snippets(State(state): State<AppState>) -> Result<Json<Value>, AppError> {
    let helpdesk = require(&state).await?;
    let snippets = helpdesk.snippets().await.map_err(map_helpdesk)?;
    Ok(Json(json!({ "snippets": snippets })))
}

async fn create_snippet(
    State(state): State<AppState>,
    Body(body): Body<SnippetBody>,
) -> Result<impl IntoResponse, AppError> {
    let helpdesk = require(&state).await?;
    let snippet = helpdesk
        .create_snippet(
            &body.shortcut,
            &body.title,
            &body.content,
            body.department_id,
        )
        .await
        .map_err(map_helpdesk)?;
    Ok((StatusCode::CREATED, Json(json!(snippet))))
}

async fn update_snippet_body(
    State(state): State<AppState>,
    Body(body): Body<SnippetBody>,
) -> Result<Json<Value>, AppError> {
    let id = body
        .id
        .ok_or_else(|| AppError::bad_request("Falta el id de la respuesta rápida."))?;
    save_snippet(&state, id, body).await
}

async fn update_snippet_path(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Body(body): Body<SnippetBody>,
) -> Result<Json<Value>, AppError> {
    save_snippet(&state, id, body).await
}

async fn delete_snippet_query(
    State(state): State<AppState>,
    Query(query): Query<IdQuery>,
) -> Result<Json<Value>, AppError> {
    remove_snippet(&state, query.id).await
}

async fn delete_snippet_path(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, AppError> {
    remove_snippet(&state, id).await
}

async fn get_rules(State(state): State<AppState>) -> Result<Json<BotSettings>, AppError> {
    let helpdesk = require(&state).await?;
    let rules = helpdesk.rules().await.map_err(map_helpdesk)?;
    Ok(Json(rules))
}

async fn put_rules(
    State(state): State<AppState>,
    Body(body): Body<BotSettings>,
) -> Result<Json<BotSettings>, AppError> {
    let helpdesk = require(&state).await?;
    let rules = helpdesk.save_rules(&body).await.map_err(map_helpdesk)?;
    Ok(Json(rules))
}

async fn save_snippet(
    state: &AppState,
    id: Uuid,
    body: SnippetBody,
) -> Result<Json<Value>, AppError> {
    let helpdesk = require(state).await?;
    let snippet = helpdesk
        .update_snippet(
            id,
            &body.shortcut,
            &body.title,
            &body.content,
            body.department_id,
        )
        .await
        .map_err(map_helpdesk)?;
    Ok(Json(json!(snippet)))
}

async fn remove_snippet(state: &AppState, id: Uuid) -> Result<Json<Value>, AppError> {
    let helpdesk = require(state).await?;
    helpdesk.delete_snippet(id).await.map_err(map_helpdesk)?;
    Ok(Json(json!({ "ok": true })))
}

fn ticket_filter(query: TicketQuery) -> Result<TicketFilter, AppError> {
    let mut filter = TicketFilter {
        department_id: query.department_id,
        ..TicketFilter::default()
    };
    match query.view.as_deref() {
        Some("mine" | "mios" | "mis_chats") => {
            filter.agent_id = Some(query.agent_id.ok_or_else(|| {
                AppError::bad_request("Selecciona un agente para ver sus chats.")
            })?);
            filter.filter_status = true;
            filter.statuses = vec![
                "en_proceso".into(),
                "esperando_agente".into(),
                "en_bot".into(),
            ];
        }
        Some("bot" | "en_bot") => {
            filter.filter_status = true;
            filter.statuses = vec!["en_bot".into()];
        }
        Some("waiting" | "esperando_agente") => {
            filter.filter_status = true;
            filter.statuses = vec!["esperando_agente".into()];
        }
        Some("resolved" | "resuelto" | "resueltos") => {
            filter.filter_status = true;
            filter.statuses = vec!["resuelto".into(), "cerrado".into()];
        }
        _ => {}
    }
    if let Some(status) = query.status.as_deref().filter(|status| !status.is_empty()) {
        filter.filter_status = true;
        filter.statuses = vec![status.to_string()];
    }
    Ok(filter)
}

fn view_json(view: ClientView) -> Json<Value> {
    Json(json!({
        "ticket": view.ticket,
        "messages": view.messages,
    }))
}

async fn require(state: &AppState) -> Result<Helpdesk, AppError> {
    state.helpdesk.get().await.map_err(AppError::unavailable)
}

fn map_helpdesk(err: HelpdeskError) -> AppError {
    match err {
        HelpdeskError::BadRequest(detail) => AppError::bad_request(detail),
        HelpdeskError::NotFound(detail) => AppError::not_found(detail),
        HelpdeskError::Conflict(detail) => AppError::conflict(detail),
        HelpdeskError::Database(detail) => {
            tracing::error!("{detail}");
            AppError::internal(detail)
        }
    }
}

async fn read_page(state: &AppState, name: &str) -> Result<Html<String>, AppError> {
    let path = state.rag.settings.templates_dir.join(name);
    let html = tokio::fs::read_to_string(&path).await.map_err(|err| {
        tracing::error!("No se pudo leer {}: {err}", path.display());
        AppError::internal("No se pudo cargar la interfaz.")
    })?;
    Ok(Html(html))
}

struct Incoming {
    ticket_id: Uuid,
    content: String,
    attachment: Option<Attachment>,
}

struct Attachment {
    bytes: Vec<u8>,
    extension: &'static str,
    kind: &'static str,
}

fn incoming_from_json(body: ClientMessageBody) -> Result<Incoming, AppError> {
    let mut mime = body.mime_type.trim().to_string();
    if mime.is_empty() {
        if let Some(from_url) = mime_from_data_url(&body.media_base64) {
            mime = from_url;
        }
    }
    let has_media = !body.media_base64.trim().is_empty();
    if !has_media {
        return Ok(Incoming {
            ticket_id: body.ticket_id,
            content: body.content,
            attachment: None,
        });
    }
    let bytes = decode_base64(&body.media_base64)?;
    let attachment = attachment_from(&mime, &body.filename, bytes)?;
    Ok(Incoming {
        ticket_id: body.ticket_id,
        content: body.content,
        attachment,
    })
}

async fn read_multipart(request: Request) -> Result<Incoming, AppError> {
    let mut multipart = Multipart::from_request(request, &())
        .await
        .map_err(|_| AppError::unprocessable())?;
    let mut ticket_id = None;
    let mut content = String::new();
    let mut mime = String::new();
    let mut filename = String::new();
    let mut bytes = Vec::new();
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|_| AppError::unprocessable())?
    {
        let name = field.name().unwrap_or("").to_string();
        match name.as_str() {
            "ticket_id" => {
                let text = field.text().await.map_err(|_| AppError::unprocessable())?;
                ticket_id = Some(
                    Uuid::parse_str(text.trim())
                        .map_err(|_| AppError::bad_request("El chat no es válido."))?,
                );
            }
            "content" => {
                content = field.text().await.map_err(|_| AppError::unprocessable())?;
            }
            "mime_type" => {
                mime = field.text().await.map_err(|_| AppError::unprocessable())?;
            }
            "file" => {
                if filename.is_empty() {
                    filename = field.file_name().unwrap_or("").to_string();
                }
                if mime.is_empty() {
                    mime = field.content_type().unwrap_or("").to_string();
                }
                bytes = field
                    .bytes()
                    .await
                    .map_err(|_| AppError::unprocessable())?
                    .to_vec();
            }
            _ => {
                let _ = field.bytes().await;
            }
        }
    }
    let ticket_id =
        ticket_id.ok_or_else(|| AppError::bad_request("Falta el identificador del chat."))?;
    if bytes.is_empty() {
        return Ok(Incoming {
            ticket_id,
            content,
            attachment: None,
        });
    }
    let attachment = attachment_from(&mime, &filename, bytes)?;
    Ok(Incoming {
        ticket_id,
        content,
        attachment,
    })
}

fn attachment_from(
    mime: &str,
    filename: &str,
    bytes: Vec<u8>,
) -> Result<Option<Attachment>, AppError> {
    let decision = media_decision(mime, filename, &bytes);
    let (extension, kind) = match decision {
        MediaDecision::Audio { extension } => (extension, "audio"),
        MediaDecision::Image { extension } => (extension, "image"),
        MediaDecision::RejectGif => return Err(AppError::bad_request(GIF_REJECTION)),
        MediaDecision::Text | MediaDecision::RejectOther => {
            return Err(AppError::unsupported(
                "Solo aceptamos texto, imágenes PNG, JPG o WebP estáticas, o notas de voz (ogg, mp3 o wav).",
            ));
        }
    };
    Ok(Some(Attachment {
        bytes,
        extension,
        kind,
    }))
}

async fn write_media(
    helpdesk: &Helpdesk,
    bytes: &[u8],
    extension: &str,
) -> Result<String, AppError> {
    if bytes.is_empty() {
        return Err(AppError::bad_request("El archivo está vacío."));
    }
    if bytes.len() > 8 * 1024 * 1024 {
        return Err(AppError::bad_request("El archivo supera 8 MB."));
    }
    let name = format!("{}.{}", Uuid::new_v4(), extension);
    let path: PathBuf = helpdesk.media_dir.join(&name);
    tokio::fs::write(&path, bytes).await.map_err(|err| {
        tracing::error!("No se pudo guardar el archivo: {err}");
        AppError::internal("No se pudo guardar el archivo.")
    })?;
    Ok(format!("/media/{name}"))
}

fn decode_base64(raw: &str) -> Result<Vec<u8>, AppError> {
    let trimmed = raw.trim();
    let payload = trimmed
        .split_once(',')
        .filter(|(prefix, _)| prefix.trim_start().starts_with("data:"))
        .map(|(_, data)| data)
        .unwrap_or(trimmed);
    base64::engine::general_purpose::STANDARD
        .decode(payload.trim())
        .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(payload.trim()))
        .map_err(|_| AppError::bad_request("No se pudo leer el archivo."))
}

fn mime_from_data_url(raw: &str) -> Option<String> {
    let rest = raw.trim().strip_prefix("data:")?;
    let (meta, _) = rest.split_once(',')?;
    let mime = meta.split(';').next()?.trim();
    if mime.is_empty() {
        None
    } else {
        Some(mime.to_string())
    }
}

const MAX_BODY: usize = 12 * 1024 * 1024;

struct Body<T>(T);

#[axum::async_trait]
impl<T, S> FromRequest<S> for Body<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = AppError;

    async fn from_request(request: Request, state: &S) -> Result<Self, Self::Rejection> {
        let bytes = Bytes::from_request(request, state)
            .await
            .map_err(|_| AppError::unprocessable())?;
        serde_json::from_slice(&bytes)
            .map(Body)
            .map_err(|_| AppError::unprocessable())
    }
}

#[derive(Debug, Deserialize)]
struct TriageBody {
    name: String,
    domain: String,
    email_micomco: String,
    reason: String,
}

#[derive(Debug, Deserialize)]
struct ClientMessageBody {
    ticket_id: Uuid,
    #[serde(default)]
    content: String,
    #[serde(default)]
    mime_type: String,
    #[serde(default)]
    filename: String,
    #[serde(default)]
    media_base64: String,
}

#[derive(Debug, Deserialize)]
struct TicketQuery {
    #[serde(default)]
    department_id: Option<Uuid>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    view: Option<String>,
    #[serde(default)]
    agent_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
struct TransferBody {
    department_id: Uuid,
    #[serde(default)]
    agent_id: Option<Uuid>,
    #[serde(default)]
    actor_agent_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
struct TimeoutBody {
    #[serde(default)]
    auto_timeout_enabled: Option<bool>,
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    agent_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
struct AgentTextBody {
    content: String,
    agent_id: Uuid,
}

#[derive(Debug, Deserialize)]
struct StatusBody {
    status: String,
    #[serde(default)]
    agent_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
struct CustomerQuery {
    #[serde(default)]
    id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
struct CustomerBody {
    id: Uuid,
    name: String,
    domain: String,
    email_micomco: String,
}

#[derive(Debug, Deserialize)]
struct SnippetBody {
    #[serde(default)]
    id: Option<Uuid>,
    shortcut: String,
    title: String,
    content: String,
    #[serde(default)]
    department_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
struct IdQuery {
    id: Uuid,
}
