//! Rutas HTTP equivalentes a `app/main.py`.

use std::collections::HashMap;

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, FromRequest, Path, Query, Request, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::de::DeserializeOwned;
use serde_json::json;
use tower_http::services::ServeDir;

use crate::error::AppError;
use crate::models::{
    ChatRequest, ChatResponse, HealthResponse, IndexStatusResponse, ReindexRequest, ReindexResponse,
};
use crate::rag::RagError;

pub use crate::state::AppState;

pub fn router(state: AppState) -> Router {
    let static_dir = state.rag.settings.static_dir.clone();
    let media_dir = state.rag.settings.media_dir.clone();
    Router::new()
        .merge(crate::helpdesk::routes())
        .route("/", get(home))
        .route("/health", get(health))
        .route("/api/chat", post(chat))
        .route("/api/reindex", post(reindex))
        .route("/api/index", get(index_status))
        .route("/api/cases", get(list_cases))
        .route("/api/conversations", get(list_conversations))
        .route("/api/conversations/:session_id", get(get_conversation))
        .nest_service("/static", ServeDir::new(static_dir))
        .nest_service("/media", ServeDir::new(media_dir))
        .layer(DefaultBodyLimit::max(12 * 1024 * 1024))
        .with_state(state)
}

struct ValidatedJson<T>(T);

#[axum::async_trait]
impl<T, S> FromRequest<S> for ValidatedJson<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = AppError;

    async fn from_request(request: Request, state: &S) -> Result<Self, Self::Rejection> {
        let bytes = Bytes::from_request(request, state)
            .await
            .map_err(|_| AppError::unprocessable())?;
        let value = serde_json::from_slice(&bytes).map_err(|_| AppError::unprocessable())?;
        Ok(ValidatedJson(value))
    }
}

async fn home(State(state): State<AppState>) -> Result<Html<String>, AppError> {
    let mut context = tera::Context::new();
    context.insert("assistant_name", "Asistente MI.COM.CO");
    let html = state
        .templates
        .render("index.html", &context)
        .map_err(|err| {
            tracing::error!("No se pudo renderizar la plantilla: {err}");
            AppError::internal("No se pudo cargar la interfaz.")
        })?;
    Ok(Html(html))
}

async fn health(State(state): State<AppState>) -> Json<HealthResponse> {
    let mut vector_status = "error";
    let mut indexed = 0usize;
    let mut overall = "degraded";
    if state.rag.vector.is_connected().await {
        vector_status = "connected";
        indexed = state.rag.vector.count().await;
        overall = "ok";
    }
    let claude_status = if state.rag.claude.is_configured() {
        "configured"
    } else {
        "missing_api_key"
    };
    Json(HealthResponse {
        status: overall.into(),
        vector_database: vector_status.into(),
        indexed_chunks: indexed,
        claude_api: claude_status.into(),
        helpdesk: if state.helpdesk.connected().await {
            "connected"
        } else {
            "disabled"
        }
        .into(),
    })
}

async fn chat(
    State(state): State<AppState>,
    ValidatedJson(payload): ValidatedJson<ChatRequest>,
) -> Result<Json<ChatResponse>, AppError> {
    let (question, session_id, history) = payload.validated()?;
    let history = history
        .into_iter()
        .map(|message| crate::conversations::HistoryTurn {
            role: message.role,
            content: message.content,
        })
        .collect::<Vec<_>>();
    match state
        .rag
        .ask(&question, &history, session_id.as_deref())
        .await
    {
        Ok(response) => Ok(Json(response)),
        Err(err) => Err(map_rag(err)),
    }
}

async fn reindex(
    State(state): State<AppState>,
    body: Bytes,
) -> Result<Json<ReindexResponse>, AppError> {
    let options = if body.trim_ascii().is_empty() {
        ReindexRequest::default()
    } else {
        serde_json::from_slice::<ReindexRequest>(&body).map_err(|_| AppError::unprocessable())?
    };
    tracing::info!(
        "Endpoint /api/reindex (reset={}, force={}).",
        options.reset,
        options.force
    );
    let result = state
        .rag
        .reindex(options.reset, options.force, None)
        .await
        .map_err(map_rag)?;
    Ok(Json(result))
}

async fn index_status(State(state): State<AppState>) -> Json<IndexStatusResponse> {
    Json(state.rag.index_status().await)
}

async fn list_cases(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<impl IntoResponse, AppError> {
    let limit = clamp_limit(params.get("limit"), 20, 100)?;
    let records = state.rag.cases.list_cases(limit);
    Ok((StatusCode::OK, Json(json!(records))))
}

async fn list_conversations(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<impl IntoResponse, AppError> {
    let limit = clamp_limit(params.get("limit"), 50, 200)?;
    let records = state
        .rag
        .conversations
        .list_conversations(limit)
        .into_iter()
        .map(|record| record.to_list_item())
        .collect::<Vec<_>>();
    Ok((StatusCode::OK, Json(json!(records))))
}

async fn get_conversation(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let Some(record) = state.rag.conversations.get(&session_id) else {
        return Err(AppError::not_found("No encontramos esa conversación."));
    };
    Ok((StatusCode::OK, Json(json!(record.to_detail()))))
}

fn clamp_limit(raw: Option<&String>, default: i64, max: i64) -> Result<usize, AppError> {
    let value = match raw {
        None => default,
        Some(text) => text.parse::<i64>().map_err(|_| AppError::unprocessable())?,
    };
    Ok(value.clamp(1, max) as usize)
}

fn map_rag(err: RagError) -> AppError {
    match err {
        RagError::BadRequest(message) => {
            tracing::error!("Solicitud rechazada: {message}");
            AppError::bad_request(message)
        }
        RagError::Document(err) => {
            tracing::error!("Error de documento: {err}");
            AppError::bad_request(err.user_message)
        }
        RagError::Vector(err) => {
            tracing::error!("Error de ChromaDB: {err}");
            AppError::unavailable(err.user_message)
        }
        RagError::Claude(err) => {
            tracing::error!("Error de Claude: {err}");
            AppError::unavailable(err.user_message)
        }
        RagError::Internal(message) => {
            tracing::error!("{message}");
            AppError::internal(message)
        }
    }
}
