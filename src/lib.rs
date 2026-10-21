//! Asistente RAG de MI.COM.CO.
//!
//! Sustituye el backend FastAPI (`run.py` / `app/`) por un servidor Axum
//! con los mismos contratos HTTP, plantilla y flujo de soporte.

pub mod cases;
pub mod chroma;
pub mod claude;
pub mod config;
pub mod conversations;
pub mod documents;
pub mod embed;
pub mod error;
pub mod helpdesk;
pub mod models;
pub mod prompts;
pub mod rag;
pub mod routes;
pub mod state;
pub mod text;
pub mod vector;

use std::sync::Arc;

use crate::cases::CaseService;
use crate::claude::{ClaudeService, SupportClient};
use crate::config::Settings;
use crate::conversations::ConversationService;
use crate::documents::DocumentService;
use crate::embed::{production_embedder, Embedder};
use crate::rag::RagService;
use crate::vector::VectorService;

/// Arma el servicio RAG de producción (embeddings reales + Claude HTTP).
pub fn build_rag(settings: Settings) -> Result<RagService, vector::VectorStoreError> {
    let embedder = production_embedder(&settings)?;
    Ok(assemble_rag(settings, embedder, None))
}

/// Permite inyectar un embedder o un cliente de Claude (pruebas).
pub fn assemble_rag(
    settings: Settings,
    embedder: Arc<dyn Embedder>,
    claude: Option<Arc<dyn SupportClient>>,
) -> RagService {
    let documents = DocumentService::from_settings(&settings);
    let vector = VectorService::new(&settings, embedder);
    let cases = CaseService::new(settings.cases_file.clone());
    let conversations = ConversationService::new(settings.conversations_directory.clone());
    let claude = claude.unwrap_or_else(|| Arc::new(ClaudeService::from_settings(&settings)));
    RagService {
        settings,
        documents,
        vector,
        claude,
        cases,
        conversations,
    }
}
