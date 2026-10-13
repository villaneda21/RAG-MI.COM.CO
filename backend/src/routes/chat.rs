use axum::extract::rejection::JsonRejection;
use axum::extract::State;
use axum::Json;

use crate::domain::models::{ChatRequest, ChatResponse};
use crate::error::AppError;
use crate::services::chat_service;
use crate::state::AppState;

pub async fn chat(
    State(state): State<AppState>,
    payload: Result<Json<ChatRequest>, JsonRejection>,
) -> Result<Json<ChatResponse>, AppError> {
    let Json(request) =
        payload.map_err(|_| AppError::Unprocessable("La solicitud no es válida.".into()))?;
    chat_service::ask(&state, request).await.map(Json)
}
