use axum::extract::{Path, Query, Request, State};
use axum::Json;

use crate::domain::models::{
    clamp_limit, IndexStatusResponse, KnowledgeChunkResponse, KnowledgeQuery, ReindexRequest,
    ReindexResponse,
};
use crate::error::AppError;
use crate::repositories::knowledge::KnowledgeRepository;
use crate::services::index_service;
use crate::state::AppState;

pub async fn index_status(
    State(state): State<AppState>,
) -> Result<Json<IndexStatusResponse>, AppError> {
    index_service::index_status(&state).await.map(Json)
}

pub async fn reindex(
    State(state): State<AppState>,
    request: Request,
) -> Result<Json<ReindexResponse>, AppError> {
    let bytes = axum::body::to_bytes(request.into_body(), 64 * 1024)
        .await
        .map_err(|_| AppError::BadRequest("No se pudo leer el cuerpo de la solicitud.".into()))?;
    let options = if bytes.iter().all(u8::is_ascii_whitespace) {
        ReindexRequest::default()
    } else {
        serde_json::from_slice::<ReindexRequest>(&bytes)
            .map_err(|_| AppError::Unprocessable("La solicitud no es válida.".into()))?
    };
    index_service::reindex(&state, options.reset, options.force)
        .await
        .map(Json)
}

pub async fn search(
    State(state): State<AppState>,
    Query(query): Query<KnowledgeQuery>,
) -> Result<Json<Vec<KnowledgeChunkResponse>>, AppError> {
    let repository = KnowledgeRepository::new(&state.pool);
    let limit = clamp_limit(query.limit, 20, 50);
    let category = query.category.as_deref();
    let chunks = match query
        .q
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(text) => repository.search(text, category, limit).await?,
        None => repository.list(category, limit).await?,
    };
    let scored = query
        .q
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty());
    Ok(Json(
        chunks
            .into_iter()
            .map(|chunk| {
                let mut response = chunk.to_knowledge();
                if !scored {
                    response.score = None;
                }
                response
            })
            .collect(),
    ))
}

pub async fn by_id(
    State(state): State<AppState>,
    Path(doc_chunk_id): Path<String>,
) -> Result<Json<Vec<KnowledgeChunkResponse>>, AppError> {
    let chunks = KnowledgeRepository::new(&state.pool)
        .by_doc_chunk_id(&doc_chunk_id)
        .await?;
    if chunks.is_empty() {
        return Err(AppError::NotFound(
            "No encontramos ese artículo en la base de conocimiento.".into(),
        ));
    }
    Ok(Json(
        chunks
            .into_iter()
            .map(|chunk| {
                let mut response = chunk.to_knowledge();
                response.score = None;
                response
            })
            .collect(),
    ))
}
