use axum::extract::{Path, Query, State};
use axum::Json;

use crate::domain::models::{clamp_limit, ConversationDetail, ConversationSummary, LimitQuery};
use crate::error::AppError;
use crate::repositories::conversations::ConversationRepository;
use crate::state::AppState;

pub async fn list(
    State(state): State<AppState>,
    Query(query): Query<LimitQuery>,
) -> Result<Json<Vec<ConversationSummary>>, AppError> {
    let limit = clamp_limit(query.limit, 50, 200);
    let records = ConversationRepository::new(&state.pool).list(limit).await?;
    Ok(Json(records))
}

pub async fn by_id(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<Json<ConversationDetail>, AppError> {
    let record = ConversationRepository::new(&state.pool)
        .get(&session_id)
        .await?
        .ok_or_else(|| AppError::NotFound("No encontramos esa conversación.".into()))?;
    Ok(Json(record))
}
