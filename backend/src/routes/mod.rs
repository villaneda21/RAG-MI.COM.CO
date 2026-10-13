pub mod cases;
pub mod chat;
pub mod conversations;
pub mod health;
pub mod knowledge;

use axum::routing::{get, post};
use axum::Router;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

use crate::state::AppState;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health::health))
        .route("/api/chat", post(chat::chat))
        .route("/api/reindex", post(knowledge::reindex))
        .route("/api/index", get(knowledge::index_status))
        .route("/api/knowledge", get(knowledge::search))
        .route("/api/knowledge/:doc_chunk_id", get(knowledge::by_id))
        .route("/api/cases", get(cases::list).post(cases::create))
        .route("/api/conversations", get(conversations::list))
        .route("/api/conversations/:session_id", get(conversations::by_id))
        .layer(TraceLayer::new_for_http())
        .layer(CorsLayer::permissive())
        .with_state(state)
}
