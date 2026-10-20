//! Configuración leída de `.env` y del entorno, equivalente a `app/config/settings.py`.

use std::path::{Path, PathBuf};

use crate::error::AppError;

const API_KEY_PLACEHOLDERS: &[&str] = &[
    "tu_api_key_aqui",
    "your_api_key_here",
    "changeme",
    "api_key",
    "sk-ant-xxxxxxxx",
];

pub fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

pub fn load_dotenv(root: &Path) {
    let _ = dotenvy::from_path(root.join(".env"));
}

pub fn ensure_env_file(root: &Path) {
    let env_path = root.join(".env");
    let example = root.join(".env.example");
    if env_path.exists() || !example.exists() {
        return;
    }
    if let Ok(text) = std::fs::read_to_string(&example) {
        if std::fs::write(&env_path, text).is_ok() {
            println!(
                "[run] Se creó .env desde .env.example. Agrega ANTHROPIC_API_KEY para chatear con Claude."
            );
        }
    }
}

/// True si hay una clave real, no el placeholder de `.env.example`.
pub fn is_usable_anthropic_key(key: &str) -> bool {
    let cleaned = key.trim();
    if cleaned.is_empty() {
        return false;
    }
    let lowered = cleaned.to_lowercase();
    if API_KEY_PLACEHOLDERS.contains(&lowered.as_str()) {
        return false;
    }
    if lowered.contains("aqui") || lowered.contains("your_api") || lowered.contains("replace") {
        return false;
    }
    true
}

#[derive(Debug, Clone)]
pub struct Settings {
    pub project_root: PathBuf,
    pub anthropic_api_key: String,
    pub claude_model: String,
    pub claude_max_tokens: u32,
    pub claude_temperature: f32,
    pub claude_fallback_models: Vec<String>,
    pub embedding_model: String,
    pub embedding_dimensions: usize,
    pub chroma_persist_directory: PathBuf,
    pub chroma_collection_name: String,
    pub chroma_url: String,
    pub chroma_tenant: String,
    pub chroma_database: String,
    pub index_meta_path: PathBuf,
    pub documents_directory: PathBuf,
    pub default_document_name: String,
    pub default_document_path: PathBuf,
    pub chunk_size_min: usize,
    pub chunk_size_max: usize,
    pub chunk_overlap: usize,
    pub retrieval_k: usize,
    pub cases_directory: PathBuf,
    pub cases_file: PathBuf,
    pub conversations_directory: PathBuf,
    pub host: String,
    pub port: u16,
}

impl Settings {
    pub fn load() -> Self {
        let root = project_root();
        let documents = root.join("data").join("documents");
        let cases = root.join("data").join("cases");
        let chroma = root.join("chroma_db");
        Self {
            anthropic_api_key: env_string("ANTHROPIC_API_KEY"),
            claude_model: env_string_or("CLAUDE_MODEL", "claude-haiku-4-5"),
            claude_max_tokens: env_parse("CLAUDE_MAX_TOKENS", 1200),
            claude_temperature: env_parse("CLAUDE_TEMPERATURE", 0.2),
            claude_fallback_models: vec![
                "claude-haiku-4-5".to_string(),
                "claude-haiku-4-5-20251001".to_string(),
            ],
            embedding_model: env_string_or("EMBEDDING_MODEL", "all-MiniLM-L6-v2"),
            embedding_dimensions: 384,
            chroma_persist_directory: chroma.clone(),
            chroma_collection_name: env_string_or(
                "CHROMA_COLLECTION_NAME",
                "empresa_knowledge_base",
            ),
            chroma_url: env_string("CHROMA_URL").trim_end_matches('/').to_string(),
            chroma_tenant: env_string_or("CHROMA_TENANT", "default_tenant"),
            chroma_database: env_string_or("CHROMA_DATABASE", "default_database"),
            index_meta_path: chroma.join(".index_meta.json"),
            default_document_name: "documento.txt".to_string(),
            default_document_path: documents.join("documento.txt"),
            documents_directory: documents,
            chunk_size_min: 800,
            chunk_size_max: 1200,
            chunk_overlap: 200,
            retrieval_k: 8,
            cases_file: cases.join("casos.jsonl"),
            conversations_directory: cases.join("conversations"),
            cases_directory: cases,
            host: env_string_or("HOST", "0.0.0.0"),
            port: env_parse("PORT", 8000),
            project_root: root,
        }
    }

    pub fn apply_bind(&mut self, host: Option<String>, port: Option<u16>) {
        if let Some(host) = host {
            let cleaned = host.trim();
            if !cleaned.is_empty() {
                self.host = cleaned.to_string();
            }
        }
        if let Some(port) = port {
            self.port = port;
        }
    }

    pub fn claude_is_configured(&self) -> bool {
        is_usable_anthropic_key(&self.anthropic_api_key)
    }

    pub fn uses_remote_chroma(&self) -> bool {
        !self.chroma_url.trim().is_empty()
    }
}

pub fn chunk_options(settings: &Settings) -> crate::text::ChunkOptions {
    crate::text::ChunkOptions {
        min: settings.chunk_size_min,
        max: settings.chunk_size_max,
        overlap: settings.chunk_overlap,
    }
}

fn env_string(key: &str) -> String {
    std::env::var(key).unwrap_or_default().trim().to_string()
}

fn env_string_or(key: &str, default: &str) -> String {
    let value = env_string(key);
    if value.is_empty() {
        default.to_string()
    } else {
        value
    }
}

fn env_parse<T: std::str::FromStr>(key: &str, default: T) -> T {
    match std::env::var(key) {
        Ok(value) => value.trim().parse().unwrap_or(default),
        Err(_) => default,
    }
}

pub fn io_document(err: std::io::Error, user_message: &str) -> AppError {
    AppError::document(err.to_string(), user_message.to_string())
}

#[cfg(test)]
mod tests {
    use super::is_usable_anthropic_key;

    #[test]
    fn placeholder_api_key_is_rejected() {
        assert!(!is_usable_anthropic_key(""));
        assert!(!is_usable_anthropic_key("tu_api_key_aqui"));
        assert!(!is_usable_anthropic_key("  your_api_key_here "));
        assert!(!is_usable_anthropic_key("sk-ant-replace-me"));
        assert!(is_usable_anthropic_key("sk-ant-api03-real-key-value"));
    }
}
