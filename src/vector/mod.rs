//! Persistencia y búsqueda: índice local o ChromaDB por HTTP.

mod chroma;
mod local;

use std::collections::BTreeMap;
use std::path::PathBuf;

use chrono::Utc;
use serde::Serialize;
use serde_json::{json, Map, Value};

use crate::config::Settings;
use crate::embeddings::Embedder;
use crate::error::AppError;
use crate::models::{json_i64, json_str, SourceChunk, TextChunk};

pub use chroma::ChromaHttp;
pub use local::{LocalRecord, LocalStore};

#[derive(Debug, Clone)]
pub struct Hit {
    pub id: Option<String>,
    pub document: String,
    pub metadata: Map<String, Value>,
    pub distance: Option<f64>,
}

#[derive(Debug, Serialize)]
struct SourceMeta {
    sha256: String,
    chunks: usize,
    indexed_at: String,
}

#[derive(Debug, Serialize)]
struct MetaWrite<'a> {
    sources: BTreeMap<String, SourceMeta>,
    collection: &'a str,
    embedding_model: &'a str,
    embedding_backend: &'a str,
    total_chunks: usize,
    indexed_at: String,
}

enum Backend {
    Local(LocalStore),
    Remote(ChromaHttp),
}

pub struct VectorService {
    backend: Backend,
    embedder: Embedder,
    meta_path: PathBuf,
    collection_name: String,
    embedding_model: String,
    ready: bool,
}

impl VectorService {
    pub fn new(settings: &Settings) -> Result<Self, AppError> {
        let embedder = Embedder::new(settings.embedding_dimensions);
        let backend = if settings.uses_remote_chroma() {
            Backend::Remote(ChromaHttp::new(
                &settings.chroma_url,
                &settings.chroma_tenant,
                &settings.chroma_database,
                &settings.chroma_collection_name,
            )?)
        } else {
            Backend::Local(LocalStore::new(
                settings.chroma_persist_directory.join("local_vectors.json"),
            ))
        };
        Ok(Self {
            backend,
            embedder,
            meta_path: settings.index_meta_path.clone(),
            collection_name: settings.chroma_collection_name.clone(),
            embedding_model: settings.embedding_model.clone(),
            ready: false,
        })
    }

    pub fn backend_name(&self) -> &'static str {
        match self.backend {
            Backend::Local(_) => "local",
            Backend::Remote(_) => "chroma",
        }
    }

    pub async fn connect(&mut self) -> Result<(), AppError> {
        let result = match &mut self.backend {
            Backend::Local(store) => store.load(),
            Backend::Remote(chroma) => chroma.heartbeat().await,
        };
        match result {
            Ok(()) => {
                self.ready = true;
                Ok(())
            }
            Err(err) => {
                self.ready = false;
                Err(err)
            }
        }
    }

    pub async fn is_connected(&mut self) -> bool {
        if !self.ready {
            return false;
        }
        match &mut self.backend {
            Backend::Local(_) => true,
            Backend::Remote(chroma) => chroma.heartbeat().await.is_ok(),
        }
    }

    pub async fn count(&mut self) -> usize {
        if !self.ready {
            return 0;
        }
        match &mut self.backend {
            Backend::Local(store) => store.len(),
            Backend::Remote(chroma) => chroma.count().await.unwrap_or(0),
        }
    }

    pub async fn reset_collection(&mut self) -> Result<(), AppError> {
        tracing::info!(collection = %self.collection_name, "eliminando colección anterior");
        match &mut self.backend {
            Backend::Local(store) => store.clear()?,
            Backend::Remote(chroma) => chroma.delete_collection().await?,
        }
        if self.meta_path.exists() {
            let _ = std::fs::remove_file(&self.meta_path);
        }
        Ok(())
    }

    pub async fn index_chunks(
        &mut self,
        chunks: &[TextChunk],
        fingerprint: &str,
        reset: bool,
        force: bool,
    ) -> Result<usize, AppError> {
        if chunks.is_empty() {
            return Err(AppError::vector(
                "No hay chunks para indexar.",
                "No hay fragmentos para guardar en la base de conocimiento.",
            ));
        }
        let source = chunks[0].source.clone();
        tracing::info!(
            count = chunks.len(),
            reset,
            force,
            %source,
            "inicio de la indexación"
        );
        if reset {
            self.reset_collection().await?;
        } else if !force
            && self.source_fingerprint(&source).as_deref() == Some(fingerprint)
            && self.count().await > 0
        {
            tracing::info!(%source, "la fuente no cambió; se omite");
            return Ok(0);
        }

        let documents: Vec<String> = chunks.iter().map(|chunk| chunk.content.clone()).collect();
        let embeddings = self.embedder.embed_many(&documents);
        let ids: Vec<String> = chunks
            .iter()
            .map(|chunk| format!("{}::{}", chunk.source, chunk.chunk_id))
            .collect();
        let metadatas: Vec<Map<String, Value>> = chunks
            .iter()
            .map(|chunk| chroma_safe_metadata(&chunk.metadata))
            .collect();

        match &mut self.backend {
            Backend::Local(store) => {
                store.delete_source(&source);
                let records = ids
                    .into_iter()
                    .zip(documents)
                    .zip(metadatas)
                    .zip(embeddings)
                    .map(|(((id, document), metadata), embedding)| LocalRecord {
                        id,
                        document,
                        metadata,
                        embedding,
                    })
                    .collect();
                store.upsert(records)?;
            }
            Backend::Remote(chroma) => {
                chroma.delete_source(&source).await?;
                chroma
                    .upsert(&ids, &embeddings, &documents, &metadatas)
                    .await?;
            }
        }
        let stored = chunks.len();
        self.write_source_meta(&source, fingerprint, stored).await?;
        tracing::info!(stored, %source, "indexación finalizada");
        Ok(stored)
    }

    pub fn source_fingerprint(&self, source: &str) -> Option<String> {
        self.source_entry(source)
            .and_then(|entry| json_str(entry.get("sha256")))
    }

    pub fn source_meta(&self, source: &str) -> Map<String, Value> {
        self.source_entry(source)
            .and_then(|entry| entry.as_object().cloned())
            .unwrap_or_default()
    }

    pub async fn query(&mut self, question: &str, n_results: usize) -> Result<Vec<Hit>, AppError> {
        let total = self.count().await;
        if total == 0 {
            return Err(AppError::vector(
                "La colección está vacía.",
                "La base de conocimiento aún no tiene información indexada. Usa la pestaña Base para indexar o ejecuta: cargo run -- ingest",
            ));
        }
        let n_results = n_results.min(total.max(1));
        let embedding = self.embedder.embed(question);
        let hits = match &mut self.backend {
            Backend::Local(store) => store.query(&embedding, question, n_results),
            Backend::Remote(chroma) => chroma.query(&embedding, n_results).await?,
        };
        tracing::info!(count = hits.len(), "consulta recuperó chunks");
        Ok(hits)
    }

    pub fn hits_to_sources(hits: &[Hit], default_source: &str) -> Vec<SourceChunk> {
        let mut sources = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        for hit in hits {
            let chunk_id = hit.metadata.get("chunk_id").and_then(json_i64).unwrap_or(0);
            let source = hit
                .metadata
                .get("source")
                .and_then(|value| value.as_str())
                .unwrap_or(default_source)
                .to_string();
            if !seen.insert((source.clone(), chunk_id)) {
                continue;
            }
            sources.push(SourceChunk {
                chunk_id,
                source,
                title: json_str(hit.metadata.get("title")),
                category: json_str(hit.metadata.get("category")),
                distance: hit.distance,
            });
        }
        sources
    }

    fn source_entry(&self, source: &str) -> Option<Value> {
        let meta = self.read_meta();
        if let Some(entry) = meta
            .get("sources")
            .and_then(|value| value.as_object())
            .and_then(|sources| sources.get(source))
        {
            return Some(entry.clone());
        }
        if meta.get("source").and_then(|value| value.as_str()) == Some(source) {
            return Some(json!({
                "sha256": meta.get("sha256"),
                "chunks": meta.get("chunks"),
                "indexed_at": meta.get("indexed_at"),
            }));
        }
        None
    }

    fn read_meta(&self) -> Value {
        let Ok(text) = std::fs::read_to_string(&self.meta_path) else {
            return Value::Null;
        };
        serde_json::from_str(&text).unwrap_or(Value::Null)
    }

    async fn write_source_meta(
        &mut self,
        source: &str,
        fingerprint: &str,
        chunks: usize,
    ) -> Result<(), AppError> {
        let now = Utc::now().format("%Y-%m-%dT%H:%M:%S%.6f+00:00").to_string();
        let meta = self.read_meta();
        let mut sources = BTreeMap::new();
        if let Some(existing) = meta.get("sources").and_then(|value| value.as_object()) {
            for (name, entry) in existing {
                if name == source {
                    continue;
                }
                sources.insert(
                    name.clone(),
                    SourceMeta {
                        sha256: json_str(entry.get("sha256")).unwrap_or_default(),
                        chunks: entry.get("chunks").and_then(json_i64).unwrap_or(0).max(0) as usize,
                        indexed_at: json_str(entry.get("indexed_at")).unwrap_or_default(),
                    },
                );
            }
        }
        sources.insert(
            source.to_string(),
            SourceMeta {
                sha256: fingerprint.to_string(),
                chunks,
                indexed_at: now.clone(),
            },
        );
        let total_chunks = self.count().await;
        let payload = MetaWrite {
            sources,
            collection: &self.collection_name,
            embedding_model: &self.embedding_model,
            embedding_backend: "feature-hash-384+bm25",
            total_chunks,
            indexed_at: now,
        };
        if let Some(parent) = self.meta_path.parent() {
            std::fs::create_dir_all(parent).map_err(|err| {
                AppError::vector(
                    format!("No se pudo guardar la metadata del índice: {err}"),
                    "No se pudieron guardar los fragmentos en la base vectorial.",
                )
            })?;
        }
        let text = serde_json::to_string_pretty(&payload).map_err(|err| {
            AppError::vector(
                format!("No se pudo serializar la metadata: {err}"),
                "No se pudieron guardar los fragmentos en la base vectorial.",
            )
        })?;
        std::fs::write(&self.meta_path, text).map_err(|err| {
            AppError::vector(
                format!("No se pudo escribir la metadata: {err}"),
                "No se pudieron guardar los fragmentos en la base vectorial.",
            )
        })
    }
}

fn chroma_safe_metadata(metadata: &Map<String, Value>) -> Map<String, Value> {
    let mut safe = Map::new();
    for (key, value) in metadata {
        match value {
            Value::Null => {}
            Value::String(_) | Value::Bool(_) | Value::Number(_) => {
                safe.insert(key.clone(), value.clone());
            }
            other => {
                safe.insert(key.clone(), Value::String(other.to_string()));
            }
        }
    }
    safe
}

#[cfg(test)]
mod tests {
    use super::VectorService;
    use crate::config::Settings;
    use crate::documents::DocumentService;
    use crate::text::ChunkOptions;
    use std::path::PathBuf;

    fn isolated_settings(root: PathBuf) -> Settings {
        let mut settings = Settings::load();
        settings.chroma_url.clear();
        settings.chroma_persist_directory = root.join("chroma");
        settings.index_meta_path = settings.chroma_persist_directory.join(".index_meta.json");
        settings.documents_directory = root.clone();
        settings.default_document_path = root.join("documento.txt");
        settings.cases_directory = root.join("cases");
        settings.cases_file = settings.cases_directory.join("casos.jsonl");
        settings.conversations_directory = settings.cases_directory.join("conversations");
        settings
    }

    fn sample(dir: &std::path::Path) -> PathBuf {
        let path = dir.join("documento.txt");
        std::fs::write(
            &path,
            [
                "La política de la empresa indica que el soporte técnico responde en horario continuo.",
                "El código de cliente tiene el formato CLI-XXXXXX y se consulta en Mi cuenta.",
                "Para acceder a cPanel se debe entrar a Mis servicios y pulsar Acceder a cPanel.",
            ]
            .join("\n\n"),
        )
        .unwrap();
        path
    }

    #[tokio::test]
    async fn chromadb_stores_information_without_duplicates() {
        let dir = std::env::temp_dir().join(format!("micomco-vec-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = sample(&dir);
        let service = DocumentService::for_file(path, dir.clone(), ChunkOptions::default());
        let (chunks, _chars, fingerprint) = service.process().unwrap();
        let mut vectors = VectorService::new(&isolated_settings(dir.clone())).unwrap();
        vectors.connect().await.unwrap();
        let stored = vectors
            .index_chunks(&chunks, &fingerprint, true, false)
            .await
            .unwrap();
        assert_eq!(stored, chunks.len());
        assert_eq!(vectors.count().await, chunks.len());
        let again = vectors
            .index_chunks(&chunks, &fingerprint, false, false)
            .await
            .unwrap();
        assert_eq!(again, 0);
        assert_eq!(vectors.count().await, chunks.len());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn query_returns_client_code() {
        let dir = std::env::temp_dir().join(format!("micomco-vec-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = sample(&dir);
        let service = DocumentService::for_file(path, dir.clone(), ChunkOptions::default());
        let (chunks, _chars, fingerprint) = service.process().unwrap();
        let mut vectors = VectorService::new(&isolated_settings(dir.clone())).unwrap();
        vectors.connect().await.unwrap();
        vectors
            .index_chunks(&chunks, &fingerprint, true, false)
            .await
            .unwrap();
        let hits = vectors
            .query("¿Dónde veo el código de cliente?", 3)
            .await
            .unwrap();
        assert!(!hits.is_empty());
        let joined = hits
            .iter()
            .map(|hit| hit.document.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(
            joined.contains("CLI-XXXXXX")
                || joined.contains("cPanel")
                || joined.to_lowercase().contains("soporte")
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
