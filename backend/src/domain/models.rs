use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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

#[derive(Debug, Serialize)]
pub struct SourceChunk {
    pub chunk_id: i32,
    pub doc_chunk_id: Option<String>,
    pub source: String,
    pub title: Option<String>,
    pub category: Option<String>,
    pub distance: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CaseSummary {
    pub case_id: String,
    pub correo: String,
    pub categoria: String,
    pub detalle: String,
    pub registrado_en: String,
    pub estado: String,
}

#[derive(Debug, Serialize)]
pub struct ChatResponse {
    pub answer: String,
    pub sources: Vec<SourceChunk>,
    pub session_id: Option<String>,
    pub case: Option<CaseSummary>,
    pub case_closed: bool,
    pub new_chat: bool,
}

#[derive(Debug, Serialize)]
pub struct CaseResponse {
    pub case_id: String,
    pub correo: String,
    pub categoria: String,
    pub detalle: String,
    pub registrado_en: String,
    pub estado: String,
    pub session_id: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateCaseRequest {
    pub correo: String,
    pub categoria: String,
    pub detalle: String,
    #[serde(default)]
    pub session_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CaseRecord {
    pub case_id: String,
    pub correo: String,
    pub categoria: String,
    pub detalle: String,
    pub registrado_en: String,
    pub estado: String,
    pub session_id: String,
}

impl CaseRecord {
    pub fn to_summary(&self) -> CaseSummary {
        CaseSummary {
            case_id: self.case_id.clone(),
            correo: self.correo.clone(),
            categoria: self.categoria.clone(),
            detalle: self.detalle.clone(),
            registrado_en: self.registrado_en.clone(),
            estado: self.estado.clone(),
        }
    }

    pub fn to_response(&self) -> CaseResponse {
        CaseResponse {
            case_id: self.case_id.clone(),
            correo: self.correo.clone(),
            categoria: self.categoria.clone(),
            detalle: self.detalle.clone(),
            registrado_en: self.registrado_en.clone(),
            estado: self.estado.clone(),
            session_id: self.session_id.clone(),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct ConversationMessage {
    pub role: String,
    pub content: String,
    pub created_at: String,
}

#[derive(Debug, Serialize)]
pub struct ConversationSummary {
    pub session_id: String,
    pub created_at: String,
    pub updated_at: String,
    pub status: String,
    pub case_id: String,
    pub correo: String,
    pub categoria: String,
    pub summary: String,
    pub message_count: i64,
}

#[derive(Debug, Serialize)]
pub struct ConversationDetail {
    pub session_id: String,
    pub created_at: String,
    pub updated_at: String,
    pub status: String,
    pub case_id: String,
    pub correo: String,
    pub categoria: String,
    pub summary: String,
    pub message_count: i64,
    pub messages: Vec<ConversationMessage>,
}

#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub status: String,
    pub vector_database: String,
    pub indexed_chunks: i64,
    pub claude_api: String,
    pub database: String,
    pub retrieval: String,
}

#[derive(Debug, Default, Deserialize)]
pub struct ReindexRequest {
    #[serde(default)]
    pub reset: bool,
    #[serde(default)]
    pub force: bool,
}

#[derive(Debug, Serialize)]
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

#[derive(Debug, Serialize)]
pub struct IndexFileStatus {
    pub name: String,
    pub status: String,
    pub chunks: i64,
    pub characters: i64,
    pub indexed_at: String,
}

#[derive(Debug, Serialize)]
pub struct IndexStatusResponse {
    pub directory: String,
    pub indexed_chunks: i64,
    pub pending_changes: bool,
    pub files: Vec<IndexFileStatus>,
}

#[derive(Debug, Clone)]
pub struct RetrievedChunk {
    pub chunk_index: i32,
    pub doc_chunk_id: Option<String>,
    pub doc_chunk_part: Option<i32>,
    pub title: String,
    pub category: String,
    pub entity: String,
    pub keywords: Vec<String>,
    pub summary: String,
    pub content: String,
    pub source: String,
    pub score: f64,
}

impl RetrievedChunk {
    pub fn to_source(&self) -> SourceChunk {
        SourceChunk {
            chunk_id: self.chunk_index,
            doc_chunk_id: self.doc_chunk_id.clone(),
            source: self.source.clone(),
            title: optional_text(&self.title),
            category: optional_text(&self.category),
            distance: Some(score_to_distance(self.score)),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct KnowledgeChunkResponse {
    pub doc_chunk_id: Option<String>,
    pub doc_chunk_part: Option<i32>,
    pub chunk_index: i32,
    pub title: String,
    pub category: String,
    pub entity: String,
    pub keywords: Vec<String>,
    pub summary: String,
    pub content: String,
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
}

impl RetrievedChunk {
    pub fn to_knowledge(&self) -> KnowledgeChunkResponse {
        KnowledgeChunkResponse {
            doc_chunk_id: self.doc_chunk_id.clone(),
            doc_chunk_part: self.doc_chunk_part,
            chunk_index: self.chunk_index,
            title: self.title.clone(),
            category: self.category.clone(),
            entity: self.entity.clone(),
            keywords: self.keywords.clone(),
            summary: self.summary.clone(),
            content: self.content.clone(),
            source: self.source.clone(),
            score: Some(self.score),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct KnowledgeQuery {
    pub q: Option<String>,
    pub category: Option<String>,
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct LimitQuery {
    pub limit: Option<i64>,
}

pub fn score_to_distance(score: f64) -> f64 {
    if score.is_finite() && score > 0.0 {
        1.0 / (1.0 + score)
    } else {
        1.0
    }
}

fn optional_text(value: &str) -> Option<String> {
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

pub fn clamp_limit(value: Option<i64>, default: i64, max: i64) -> i64 {
    match value {
        Some(limit) => limit.clamp(1, max),
        None => default,
    }
}
