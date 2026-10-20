//! Configuración leída de `.env` y del entorno, equivalente a `app/config/settings.py`.

use std::env;
use std::path::{Path, PathBuf};

const API_KEY_PLACEHOLDERS: &[&str] = &[
    "tu_api_key_aqui",
    "your_api_key_here",
    "changeme",
    "api_key",
    "sk-ant-xxxxxxxx",
];

pub const CLAUDE_FALLBACK_MODELS: &[&str] = &["claude-haiku-4-5", "claude-haiku-4-5-20251001"];

/// True si hay una clave real, no el placeholder de `.env.example`.
pub fn is_usable_anthropic_key(key: Option<&str>) -> bool {
    let cleaned = key.unwrap_or("").trim();
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
    pub embedding_model: String,
    pub chroma_persist_directory: PathBuf,
    pub chroma_collection_name: String,
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
    pub chroma_host: Option<String>,
    pub chroma_tenant: String,
    pub chroma_database: String,
    pub chroma_token: Option<String>,
    pub fastembed_cache: PathBuf,
    pub static_dir: PathBuf,
    pub templates_dir: PathBuf,
    pub database_url: String,
    pub media_dir: PathBuf,
}

impl Settings {
    pub fn load() -> Self {
        let root = find_project_root();
        ensure_env_file(&root);
        let _ = dotenvy::from_path(root.join(".env"));
        Self::from_env(root)
    }

    pub fn from_env(project_root: PathBuf) -> Self {
        let documents_directory = project_root.join("data").join("documents");
        let default_document_name = "documento.txt".to_string();
        let chroma_persist_directory = project_root.join("chroma_db");
        let cases_directory = project_root.join("data").join("cases");
        let chroma_host = env_nonempty("CHROMA_HOST");
        Self {
            anthropic_api_key: env::var("ANTHROPIC_API_KEY")
                .unwrap_or_default()
                .trim()
                .to_string(),
            claude_model: env_string("CLAUDE_MODEL", "claude-haiku-4-5"),
            claude_max_tokens: env_u32("CLAUDE_MAX_TOKENS", 1200),
            claude_temperature: env_f32("CLAUDE_TEMPERATURE", 0.2),
            embedding_model: env_string("EMBEDDING_MODEL", "all-MiniLM-L6-v2"),
            chroma_collection_name: env_string("CHROMA_COLLECTION_NAME", "empresa_knowledge_base"),
            index_meta_path: chroma_persist_directory.join(".index_meta.json"),
            default_document_path: documents_directory.join(&default_document_name),
            default_document_name,
            documents_directory,
            chroma_persist_directory,
            chunk_size_min: 800,
            chunk_size_max: 1200,
            chunk_overlap: 200,
            retrieval_k: 8,
            cases_file: cases_directory.join("casos.jsonl"),
            conversations_directory: cases_directory.join("conversations"),
            cases_directory,
            host: env_string("HOST", "0.0.0.0"),
            port: env_u16("PORT", 8000),
            chroma_tenant: env_string("CHROMA_TENANT", "default_tenant"),
            chroma_database: env_string("CHROMA_DATABASE", "default_database"),
            chroma_token: env_nonempty("CHROMA_TOKEN"),
            fastembed_cache: project_root.join(".cache").join("fastembed"),
            static_dir: project_root.join("static"),
            templates_dir: project_root.join("templates"),
            database_url: env_string(
                "DATABASE_URL",
                "postgres://rag:rag@127.0.0.1:5432/rag_micomco?sslmode=disable",
            ),
            media_dir: project_root.join("data").join("media"),
            chroma_host,
            project_root,
        }
    }

    /// Ajustes aislados para pruebas: documentos y vectores viven en `data_root`.
    pub fn for_tests(data_root: &Path) -> Self {
        let mut settings = Self::from_env(PathBuf::from(env!("CARGO_MANIFEST_DIR")));
        settings.documents_directory = data_root.to_path_buf();
        settings.default_document_path = data_root.join("documento.txt");
        settings.chroma_persist_directory = data_root.join("chroma_db");
        settings.index_meta_path = data_root.join("chroma_db").join(".index_meta.json");
        settings.cases_directory = data_root.join("cases");
        settings.cases_file = data_root.join("cases").join("casos.jsonl");
        settings.conversations_directory = data_root.join("cases").join("conversations");
        settings.chroma_host = None;
        settings.anthropic_api_key.clear();
        settings.chroma_collection_name = "test_knowledge_base".to_string();
        settings
    }

    pub fn claude_is_configured(&self) -> bool {
        is_usable_anthropic_key(Some(&self.anthropic_api_key))
    }
}

pub fn ensure_env_file(root: &Path) {
    let env_path = root.join(".env");
    let example = root.join(".env.example");
    if env_path.exists() || !example.exists() {
        return;
    }
    if std::fs::copy(&example, &env_path).is_ok() {
        tracing::info!(
            "Se creó .env desde .env.example. Agrega ANTHROPIC_API_KEY para chatear con Claude."
        );
    }
}

fn find_project_root() -> PathBuf {
    if let Ok(root) = env::var("RAG_PROJECT_ROOT") {
        let path = PathBuf::from(root);
        if path.join("templates").join("index.html").exists() {
            return path;
        }
    }
    let mut candidates = Vec::new();
    candidates.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")));
    if let Ok(cwd) = env::current_dir() {
        candidates.push(cwd);
    }
    if let Ok(exe) = env::current_exe() {
        if let Some(parent) = exe.parent() {
            candidates.push(parent.to_path_buf());
        }
    }
    for start in candidates {
        let mut current = Some(start.as_path());
        while let Some(dir) = current {
            if dir.join("templates").join("index.html").exists() && dir.join("static").is_dir() {
                return dir.to_path_buf();
            }
            current = dir.parent();
        }
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn env_string(name: &str, default: &str) -> String {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| default.to_string())
}

fn env_nonempty(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn env_u32(name: &str, default: u32) -> u32 {
    env::var(name)
        .ok()
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(default)
}

fn env_u16(name: &str, default: u16) -> u16 {
    env::var(name)
        .ok()
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(default)
}

fn env_f32(name: &str, default: f32) -> f32 {
    env::var(name)
        .ok()
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholder_api_key_is_rejected() {
        assert!(!is_usable_anthropic_key(Some("")));
        assert!(!is_usable_anthropic_key(None));
        assert!(!is_usable_anthropic_key(Some("tu_api_key_aqui")));
        assert!(!is_usable_anthropic_key(Some("  your_api_key_here  ")));
        assert!(is_usable_anthropic_key(Some("sk-ant-api03-realish-value")));
    }
}
