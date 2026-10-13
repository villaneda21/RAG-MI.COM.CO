use std::path::{Path, PathBuf};

#[derive(Clone)]
pub struct Config {
    pub database_url: String,
    pub host: String,
    pub port: u16,
    pub documents_dir: PathBuf,
    pub fallback_document: PathBuf,
    pub retrieval_k: i64,
    pub chunk_size_min: usize,
    pub chunk_size_max: usize,
    pub chunk_overlap: usize,
}

impl std::fmt::Debug for Config {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Config")
            .field("database_url", &"(oculto)")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("documents_dir", &self.documents_dir)
            .field("fallback_document", &self.fallback_document)
            .field("retrieval_k", &self.retrieval_k)
            .finish()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("Falta la variable de entorno {0}.")]
    Missing(&'static str),
    #[error("La variable {0} no es válida: {1}")]
    Invalid(&'static str, String),
}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        let database_url = required("DATABASE_URL")?;
        if !(database_url.starts_with("postgres://") || database_url.starts_with("postgresql://")) {
            return Err(ConfigError::Invalid(
                "DATABASE_URL",
                "debe comenzar por postgres:// o postgresql://".into(),
            ));
        }

        let root = project_root();
        let documents_dir =
            env_path("DOCUMENTS_DIR").unwrap_or_else(|| root.join("data/documents"));
        let fallback_document = env_path("FALLBACK_DOCUMENT")
            .unwrap_or_else(|| root.join("base_de_conocimiento_micomco_rag.txt"));
        let chunk_size_min = env_usize("CHUNK_SIZE_MIN", 800)?;
        let chunk_size_max = env_usize("CHUNK_SIZE_MAX", 1200)?;
        let chunk_overlap = env_usize("CHUNK_OVERLAP", 200)?;
        if chunk_size_min == 0 || chunk_size_max < chunk_size_min || chunk_overlap > chunk_size_max
        {
            return Err(ConfigError::Invalid(
                "CHUNK_SIZE_MIN/MAX",
                "se requiere 0 < mínimo <= máximo y un solapamiento menor o igual al máximo".into(),
            ));
        }

        Ok(Self {
            database_url,
            host: env_string("HOST", "0.0.0.0"),
            port: env_u16("PORT", 8000)?,
            documents_dir,
            fallback_document,
            retrieval_k: env_i64("RETRIEVAL_K", 8)?.clamp(1, 20),
            chunk_size_min,
            chunk_size_max,
            chunk_overlap,
        })
    }

    pub fn for_tests(database_url: String) -> Self {
        let root = project_root();
        Self {
            database_url,
            host: "127.0.0.1".into(),
            port: 0,
            documents_dir: root.join("data/documents"),
            fallback_document: root.join("base_de_conocimiento_micomco_rag.txt"),
            retrieval_k: 8,
            chunk_size_min: 800,
            chunk_size_max: 1200,
            chunk_overlap: 200,
        }
    }
}

pub fn project_root() -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest.parent().map(Path::to_path_buf).unwrap_or(manifest)
}

pub fn load_env() {
    let root_env = project_root().join(".env");
    let _ = dotenvy::from_path(root_env);
    let _ = dotenvy::dotenv();
}

fn required(name: &'static str) -> Result<String, ConfigError> {
    match std::env::var(name) {
        Ok(value) if !value.trim().is_empty() => Ok(value),
        _ => Err(ConfigError::Missing(name)),
    }
}

fn env_string(name: &str, default: &str) -> String {
    match std::env::var(name) {
        Ok(value) if !value.trim().is_empty() => value,
        _ => default.to_string(),
    }
}

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(PathBuf::from)
}

fn env_u16(name: &'static str, default: u16) -> Result<u16, ConfigError> {
    match std::env::var(name) {
        Ok(value) if !value.trim().is_empty() => value
            .trim()
            .parse::<u16>()
            .map_err(|_| ConfigError::Invalid(name, value)),
        _ => Ok(default),
    }
}

fn env_i64(name: &'static str, default: i64) -> Result<i64, ConfigError> {
    match std::env::var(name) {
        Ok(value) if !value.trim().is_empty() => value
            .trim()
            .parse::<i64>()
            .map_err(|_| ConfigError::Invalid(name, value)),
        _ => Ok(default),
    }
}

fn env_usize(name: &'static str, default: usize) -> Result<usize, ConfigError> {
    match std::env::var(name) {
        Ok(value) if !value.trim().is_empty() => value
            .trim()
            .parse::<usize>()
            .map_err(|_| ConfigError::Invalid(name, value)),
        _ => Ok(default),
    }
}
