use axum::extract::rejection::JsonRejection;
use axum::extract::{Query, State};
use axum::Json;

use crate::domain::models::clamp_limit;
use crate::domain::models::{CaseResponse, CreateCaseRequest, LimitQuery};
use crate::error::AppError;
use crate::repositories::cases::CaseRepository;
use crate::services::case_service;
use crate::state::AppState;

pub async fn list(
    State(state): State<AppState>,
    Query(query): Query<LimitQuery>,
) -> Result<Json<Vec<CaseResponse>>, AppError> {
    let limit = clamp_limit(query.limit, 20, 100);
    let records = CaseRepository::new(&state.pool).list(limit).await?;
    Ok(Json(
        records
            .into_iter()
            .map(|record| record.to_response())
            .collect(),
    ))
}

pub async fn create(
    State(state): State<AppState>,
    payload: Result<Json<CreateCaseRequest>, JsonRejection>,
) -> Result<Json<CaseResponse>, AppError> {
    let Json(request) =
        payload.map_err(|_| AppError::Unprocessable("La solicitud no es válida.".into()))?;
    case_service::create_case(&state, request).await.map(Json)
}
