//! Contratos JSON de la API. Equivalen a los modelos Pydantic de `app/models/schemas.py`.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::error::AppError;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
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

#[derive(Debug, Clone, Deserialize)]
pub struct ChatRequest {
    pub question: String,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub history: Vec<ChatMessage>,
}

impl ChatRequest {
    pub fn validate(&mut self) -> Result<(), AppError> {
        let cleaned = self.question.trim();
        if cleaned.is_empty() || cleaned.chars().count() > 2000 {
            return Err(AppError::unprocessable("La solicitud no es válida."));
        }
        self.question = cleaned.to_string();
        for message in &self.history {
            if message.content.is_empty() {
                return Err(AppError::unprocessable("La solicitud no es válida."));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SourceChunk {
    pub chunk_id: i64,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub distance: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CaseSummary {
    pub case_id: String,
    pub correo: String,
    pub categoria: String,
    pub detalle: String,
    pub registrado_en: String,
    pub estado: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatResponse {
    pub answer: String,
    pub sources: Vec<SourceChunk>,
    pub session_id: Option<String>,
    pub case: Option<CaseSummary>,
    pub case_closed: bool,
    pub new_chat: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConversationMessage {
    pub role: String,
    pub content: String,
    #[serde(default)]
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConversationSummary {
    pub session_id: String,
    pub created_at: String,
    pub updated_at: String,
    pub status: String,
    pub case_id: String,
    pub correo: String,
    pub categoria: String,
    pub summary: String,
    pub message_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConversationDetail {
    #[serde(flatten)]
    pub summary: ConversationSummary,
    pub messages: Vec<ConversationMessage>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HealthResponse {
    pub status: String,
    pub vector_database: String,
    pub indexed_chunks: i64,
    pub claude_api: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ReindexRequest {
    #[serde(default)]
    pub reset: bool,
    #[serde(default)]
    pub force: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReindexResponse {
    pub status: String,
    pub message: String,
    pub chunks_indexed: i64,
    pub characters: i64,
    pub source: String,
    pub sources: Vec<String>,
    pub files_indexed: i64,
    pub files_skipped: i64,
    pub reset: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct IndexFileStatus {
    pub name: String,
    pub status: String,
    pub chunks: i64,
    pub characters: i64,
    pub indexed_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct IndexStatusResponse {
    pub directory: String,
    pub indexed_chunks: i64,
    pub pending_changes: bool,
    pub files: Vec<IndexFileStatus>,
}

pub fn json_i64(value: &Value) -> Option<i64> {
    match value {
        Value::Number(number) => number
            .as_i64()
            .or_else(|| number.as_u64().and_then(|v| i64::try_from(v).ok()))
            .or_else(|| number.as_f64().map(|v| v as i64)),
        Value::String(text) => text.parse().ok(),
        _ => None,
    }
}

pub fn json_str(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(text) => {
            if text.is_empty() {
                None
            } else {
                Some(text.clone())
            }
        }
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}
