//! Contratos JSON de la API, equivalentes a los modelos Pydantic.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::error::AppError;

#[derive(Debug, Clone)]
pub struct TextChunk {
    pub content: String,
    pub chunk_id: i64,
    pub source: String,
    pub metadata: Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Deserialize)]
pub struct ChatRequest {
    pub question: String,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub history: Vec<ChatMessage>,
}

impl ChatRequest {
    /// Aplica las mismas reglas que el validador de Pydantic.
    /// Una pregunta vacía o demasiado larga responde 422, como el handler de ValidationError.
    pub fn validated(self) -> Result<(String, Option<String>, Vec<ChatMessage>), AppError> {
        let cleaned = self.question.trim().to_string();
        if cleaned.is_empty() || cleaned.chars().count() > 2000 {
            return Err(AppError::unprocessable());
        }
        for message in &self.history {
            if message.content.is_empty() {
                return Err(AppError::unprocessable());
            }
        }
        Ok((cleaned, self.session_id, self.history))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceChunk {
    pub chunk_id: i64,
    pub source: String,
    pub title: Option<String>,
    pub category: Option<String>,
    pub distance: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaseSummary {
    pub case_id: String,
    pub correo: String,
    pub categoria: String,
    pub detalle: String,
    pub registrado_en: String,
    pub estado: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChatResponse {
    pub answer: String,
    pub sources: Vec<SourceChunk>,
    pub session_id: Option<String>,
    pub case: Option<CaseSummary>,
    pub case_closed: bool,
    pub new_chat: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthResponse {
    pub status: String,
    pub vector_database: String,
    pub indexed_chunks: usize,
    pub claude_api: String,
    pub helpdesk: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ReindexRequest {
    #[serde(default)]
    pub reset: bool,
    #[serde(default)]
    pub force: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReindexResponse {
    pub status: String,
    pub message: String,
    pub chunks_indexed: usize,
    pub characters: usize,
    pub source: String,
    pub sources: Vec<String>,
    pub files_indexed: usize,
    pub files_skipped: usize,
    pub reset: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct IndexFileStatus {
    pub name: String,
    pub status: String,
    pub chunks: i64,
    pub characters: usize,
    pub indexed_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct IndexStatusResponse {
    pub directory: String,
    pub indexed_chunks: usize,
    pub pending_changes: bool,
    pub files: Vec<IndexFileStatus>,
}

#[derive(Debug, Clone)]
pub struct Hit {
    pub id: Option<String>,
    pub document: String,
    pub metadata: Map<String, Value>,
    pub distance: Option<f64>,
}
