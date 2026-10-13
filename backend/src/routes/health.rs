use axum::extract::State;
use axum::Json;

use crate::domain::models::HealthResponse;
use crate::repositories::knowledge::KnowledgeRepository;
use crate::state::AppState;

pub async fn health(State(state): State<AppState>) -> Json<HealthResponse> {
    match KnowledgeRepository::new(&state.pool).count_chunks().await {
        Ok(indexed_chunks) => Json(HealthResponse {
            status: "ok".into(),
            vector_database: "connected".into(),
            indexed_chunks,
            claude_api: "replaced_by_grounded_composer".into(),
            database: "postgres".into(),
            retrieval: "postgresql_fts_trgm".into(),
        }),
        Err(err) => {
            tracing::error!(error = %err, "health check falló");
            Json(HealthResponse {
                status: "degraded".into(),
                vector_database: "error".into(),
                indexed_chunks: 0,
                claude_api: "replaced_by_grounded_composer".into(),
                database: "postgres".into(),
                retrieval: "postgresql_fts_trgm".into(),
            })
        }
    }
}
