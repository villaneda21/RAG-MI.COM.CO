//! Índice vectorial: SQLite local (por defecto) o ChromaDB REST si `CHROMA_HOST` está definido.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use chrono::Utc;
use rusqlite::{params, Connection};
use serde_json::{json, Map, Value};

use crate::chroma::ChromaClient;
use crate::config::Settings;
use crate::embed::Embedder;
use crate::models::{Hit, SourceChunk, TextChunk};

#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct VectorStoreError {
    pub message: String,
    pub user_message: String,
}

impl VectorStoreError {
    pub fn new(message: impl Into<String>, user_message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            user_message: user_message.into(),
        }
    }
}

struct VectorInner {
    collection_name: String,
    embedding_model: String,
    default_document: String,
    embedder: Arc<dyn Embedder>,
    meta_path: PathBuf,
    meta_lock: Mutex<()>,
    db_path: PathBuf,
    local: Mutex<Option<Connection>>,
    remote: Option<ChromaClient>,
    last_query: Mutex<Option<QueryObservation>>,
}

#[derive(Debug, Clone)]
pub struct QueryObservation {
    pub elapsed_ms: f64,
    pub top_distance: Option<f64>,
    pub top_chunk_id: Option<String>,
}

#[derive(Clone)]
pub struct VectorService {
    inner: Arc<VectorInner>,
}

impl VectorService {
    pub fn new(settings: &Settings, embedder: Arc<dyn Embedder>) -> Self {
        let remote = match settings.chroma_host.as_deref() {
            Some(host) => match ChromaClient::new(
                host,
                &settings.chroma_tenant,
                &settings.chroma_database,
                &settings.chroma_collection_name,
                settings.chroma_token.as_deref(),
            ) {
                Ok(client) => {
                    tracing::info!("Vector store: ChromaDB REST en {host}");
                    Some(client)
                }
                Err(err) => {
                    tracing::error!("No se pudo preparar el cliente de ChromaDB: {err}");
                    None
                }
            },
            None => {
                tracing::info!(
                    "Vector store local en {}",
                    settings.chroma_persist_directory.display()
                );
                None
            }
        };
        Self {
            inner: Arc::new(VectorInner {
                collection_name: settings.chroma_collection_name.clone(),
                embedding_model: settings.embedding_model.clone(),
                default_document: settings.default_document_name.clone(),
                embedder,
                meta_path: settings.index_meta_path.clone(),
                meta_lock: Mutex::new(()),
                db_path: settings
                    .chroma_persist_directory
                    .join("rust_vectors.sqlite"),
                local: Mutex::new(None),
                remote,
                last_query: Mutex::new(None),
            }),
        }
    }

    pub fn clear_last_query(&self) {
        *self
            .inner
            .last_query
            .lock()
            .unwrap_or_else(|err| err.into_inner()) = None;
    }

    pub fn last_query(&self) -> Option<QueryObservation> {
        self.inner
            .last_query
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .clone()
    }

    pub fn collection_name(&self) -> &str {
        &self.inner.collection_name
    }

    pub async fn connect(&self) -> Result<(), VectorStoreError> {
        if let Some(remote) = &self.inner.remote {
            remote.heartbeat().await?;
            tracing::info!("ChromaDB conectado en {}", self.inner.db_path.display());
            return Ok(());
        }
        let inner = Arc::clone(&self.inner);
        tokio::task::spawn_blocking(move || open_local(&inner))
            .await
            .map_err(|err| {
                VectorStoreError::new(
                    format!("Error al conectar con el índice local: {err}"),
                    "No se pudo conectar con la base vectorial. Verifica que chroma_db/ sea accesible.",
                )
            })?
    }

    pub async fn is_connected(&self) -> bool {
        self.connect().await.is_ok()
    }

    pub async fn count(&self) -> usize {
        if let Some(remote) = &self.inner.remote {
            return remote.count().await.unwrap_or(0);
        }
        let inner = Arc::clone(&self.inner);
        tokio::task::spawn_blocking(move || {
            let guard = inner.local.lock().unwrap_or_else(|err| err.into_inner());
            let Some(conn) = guard.as_ref() else {
                return 0;
            };
            local_count(conn)
        })
        .await
        .unwrap_or(0)
    }

    pub async fn reset_collection(&self) -> Result<(), VectorStoreError> {
        tracing::info!(
            "Eliminando colección anterior: {}",
            self.inner.collection_name
        );
        if let Some(remote) = &self.inner.remote {
            remote.reset_collection().await?;
        } else {
            let inner = Arc::clone(&self.inner);
            tokio::task::spawn_blocking(move || {
                open_local(&inner)?;
                let mut guard = inner.local.lock().unwrap_or_else(|err| err.into_inner());
                let conn = guard.as_mut().ok_or_else(|| {
                    VectorStoreError::new(
                        "Índice local no abierto.",
                        "No se pudo conectar con la base vectorial. Verifica que chroma_db/ sea accesible.",
                    )
                })?;
                conn.execute("DELETE FROM chunks", [])
                    .map_err(sqlite_error)?;
                Ok::<(), VectorStoreError>(())
            })
            .await
            .map_err(|err| {
                VectorStoreError::new(
                    err.to_string(),
                    "No se pudieron guardar los fragmentos en la base vectorial.",
                )
            })??;
        }
        let _guard = self
            .inner
            .meta_lock
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        if self.inner.meta_path.exists() {
            let _ = fs::remove_file(&self.inner.meta_path);
        }
        tracing::info!("Colección eliminada. Lista para recrearse desde cero.");
        Ok(())
    }

    pub async fn preload_embeddings(&self) -> Result<(), VectorStoreError> {
        self.inner.embedder.embed(&["listo".to_string()]).await?;
        Ok(())
    }

    pub fn source_fingerprint(&self, source: &str) -> Option<String> {
        let meta = self.source_meta(source);
        meta.get("sha256")
            .and_then(|value| value.as_str())
            .map(str::to_string)
    }

    pub fn source_meta(&self, source: &str) -> Map<String, Value> {
        let _guard = self
            .inner
            .meta_lock
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        let meta = normalized_meta(&self.inner.meta_path);
        meta.get("sources")
            .and_then(|value| value.as_object())
            .and_then(|sources| sources.get(source))
            .and_then(|value| value.as_object())
            .cloned()
            .unwrap_or_default()
    }

    pub async fn index_chunks(
        &self,
        chunks: &[TextChunk],
        fingerprint: &str,
        reset: bool,
        force: bool,
    ) -> Result<usize, VectorStoreError> {
        if chunks.is_empty() {
            return Err(VectorStoreError::new(
                "No hay chunks para indexar.",
                "No hay fragmentos para guardar en la base de conocimiento.",
            ));
        }
        tracing::info!(
            "Inicio de la indexación ({} chunks, reset={reset}, force={force}).",
            chunks.len()
        );
        let source = chunks[0].source.clone();
        if reset {
            self.reset_collection().await?;
        } else if !force
            && self.source_fingerprint(&source).as_deref() == Some(fingerprint)
            && self.count().await > 0
        {
            tracing::info!(
                "La fuente {source} no cambió (hash={}). Se omite.",
                &fingerprint[..12.min(fingerprint.len())]
            );
            return Ok(0);
        }

        let documents: Vec<String> = chunks.iter().map(|chunk| chunk.content.clone()).collect();
        let embeddings = self.inner.embedder.embed(&documents).await?;
        if embeddings.len() != chunks.len() {
            return Err(VectorStoreError::new(
                "El modelo devolvió una cantidad distinta de embeddings.",
                "No se pudieron generar los embeddings del documento. Revisa el modelo configurado.",
            ));
        }
        let ids: Vec<String> = chunks
            .iter()
            .map(|chunk| format!("{}::{}", chunk.source, chunk.chunk_id))
            .collect();
        let metadatas: Vec<Map<String, Value>> = chunks
            .iter()
            .map(|chunk| chroma_safe_metadata(&chunk.metadata))
            .collect();

        if let Some(remote) = &self.inner.remote {
            remote.delete_source(&source).await?;
            remote
                .upsert(&ids, &documents, &metadatas, &embeddings)
                .await?;
        } else {
            let inner = Arc::clone(&self.inner);
            let source_for_delete = source.clone();
            tokio::task::spawn_blocking(move || {
                open_local(&inner)?;
                let mut guard = inner.local.lock().unwrap_or_else(|err| err.into_inner());
                let conn = guard.as_mut().ok_or_else(|| {
                    VectorStoreError::new(
                        "Índice local no abierto.",
                        "No se pudo conectar con la base vectorial. Verifica que chroma_db/ sea accesible.",
                    )
                })?;
                local_replace(conn, &source_for_delete, &ids, &documents, &metadatas, &embeddings)
            })
            .await
            .map_err(|err| {
                VectorStoreError::new(
                    err.to_string(),
                    "No se pudieron guardar los fragmentos en la base vectorial.",
                )
            })??;
        }

        let total = self.count().await;
        self.write_source_meta(&source, fingerprint, chunks.len(), total)?;
        tracing::info!(
            "Indexación finalizada: {} chunks de '{source}'.",
            chunks.len()
        );
        Ok(chunks.len())
    }

    pub async fn query(
        &self,
        question: &str,
        n_results: usize,
    ) -> Result<Vec<Hit>, VectorStoreError> {
        let started = std::time::Instant::now();
        let outcome = self.query_timed(question, n_results).await;
        let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
        let (top_distance, top_chunk_id) = outcome
            .as_ref()
            .ok()
            .and_then(|hits| hits.first())
            .map(|hit| (hit.distance, meta_string(&hit.metadata, "doc_chunk_id")))
            .unwrap_or((None, None));
        *self
            .inner
            .last_query
            .lock()
            .unwrap_or_else(|err| err.into_inner()) = Some(QueryObservation {
            elapsed_ms,
            top_distance,
            top_chunk_id,
        });
        outcome
    }

    async fn query_timed(
        &self,
        question: &str,
        n_results: usize,
    ) -> Result<Vec<Hit>, VectorStoreError> {
        let n_results = n_results.max(1);
        let total = self.count().await;
        if total == 0 {
            return Err(VectorStoreError::new(
                "La colección está vacía.",
                "La base de conocimiento aún no tiene información indexada. Ejecuta cargo run --bin ingest o usa la pestaña Base para indexar.",
            ));
        }
        let n = n_results.min(total.max(1));
        let vectors = self.inner.embedder.embed(&[question.to_string()]).await?;
        let query_vector = vectors.into_iter().next().ok_or_else(|| {
            VectorStoreError::new(
                "El modelo no devolvió un embedding para la consulta.",
                "No se pudieron generar los embeddings del documento. Revisa el modelo configurado.",
            )
        })?;
        let hits = if let Some(remote) = &self.inner.remote {
            remote.query(&query_vector, n).await?
        } else {
            let inner = Arc::clone(&self.inner);
            tokio::task::spawn_blocking(move || local_query(&inner, &query_vector, n))
                .await
                .map_err(|err| {
                    VectorStoreError::new(
                        err.to_string(),
                        "No se pudo buscar en la base de conocimiento. Inténtalo de nuevo.",
                    )
                })??
        };
        tracing::info!("Consulta recuperó {} chunks.", hits.len());
        Ok(hits)
    }

    pub fn hits_to_sources(&self, hits: &[Hit]) -> Vec<SourceChunk> {
        let mut sources = Vec::new();
        let mut seen = HashSet::new();
        for hit in hits {
            let chunk_id = meta_i64(&hit.metadata, "chunk_id").unwrap_or(0);
            let source = meta_string(&hit.metadata, "source")
                .unwrap_or_else(|| self.inner.default_document.clone());
            if !seen.insert((source.clone(), chunk_id)) {
                continue;
            }
            sources.push(SourceChunk {
                chunk_id,
                source,
                title: meta_string(&hit.metadata, "title"),
                category: meta_string(&hit.metadata, "category"),
                distance: hit.distance,
            });
        }
        sources
    }

    fn write_source_meta(
        &self,
        source: &str,
        fingerprint: &str,
        chunks: usize,
        total_chunks: usize,
    ) -> Result<(), VectorStoreError> {
        let _guard = self
            .inner
            .meta_lock
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        let now = Utc::now().to_rfc3339();
        let meta = normalized_meta(&self.inner.meta_path);
        let mut sources = meta
            .get("sources")
            .and_then(|value| value.as_object())
            .cloned()
            .unwrap_or_default();
        sources.insert(
            source.to_string(),
            json!({
                "sha256": fingerprint,
                "chunks": chunks,
                "indexed_at": now,
            }),
        );
        let payload = json!({
            "sources": sources,
            "collection": self.inner.collection_name,
            "embedding_model": self.inner.embedding_model,
            "total_chunks": total_chunks,
            "indexed_at": now,
        });
        if let Some(parent) = self.inner.meta_path.parent() {
            fs::create_dir_all(parent).map_err(|err| {
                VectorStoreError::new(
                    format!("No se pudo crear chroma_db: {err}"),
                    "No se pudieron guardar los fragmentos en la base vectorial.",
                )
            })?;
        }
        fs::write(
            &self.inner.meta_path,
            serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "{}".into()),
        )
        .map_err(|err| {
            VectorStoreError::new(
                format!("No se pudo escribir .index_meta.json: {err}"),
                "No se pudieron guardar los fragmentos en la base vectorial.",
            )
        })
    }
}

fn open_local(inner: &VectorInner) -> Result<(), VectorStoreError> {
    let mut guard = inner.local.lock().unwrap_or_else(|err| err.into_inner());
    if guard.is_some() {
        return Ok(());
    }
    if let Some(parent) = inner.db_path.parent() {
        fs::create_dir_all(parent).map_err(|err| {
            VectorStoreError::new(
                format!("No se pudo crear chroma_db: {err}"),
                "No se pudo conectar con la base vectorial. Verifica que chroma_db/ sea accesible.",
            )
        })?;
    }
    let conn = Connection::open(&inner.db_path).map_err(|err| {
        VectorStoreError::new(
            format!("Error al conectar con el índice local: {err}"),
            "No se pudo conectar con la base vectorial. Verifica que chroma_db/ sea accesible.",
        )
    })?;
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         CREATE TABLE IF NOT EXISTS chunks (
            id TEXT PRIMARY KEY,
            source TEXT NOT NULL,
            document TEXT NOT NULL,
            metadata TEXT NOT NULL,
            embedding BLOB NOT NULL
         );
         CREATE INDEX IF NOT EXISTS idx_chunks_source ON chunks(source);",
    )
    .map_err(sqlite_error)?;
    tracing::info!("Índice local conectado en {}", inner.db_path.display());
    *guard = Some(conn);
    Ok(())
}

fn local_count(conn: &Connection) -> usize {
    conn.query_row("SELECT COUNT(*) FROM chunks", [], |row| {
        row.get::<_, i64>(0)
    })
    .unwrap_or(0)
    .max(0) as usize
}

fn local_replace(
    conn: &mut Connection,
    source: &str,
    ids: &[String],
    documents: &[String],
    metadatas: &[Map<String, Value>],
    embeddings: &[Vec<f32>],
) -> Result<(), VectorStoreError> {
    conn.execute("DELETE FROM chunks WHERE source = ?1", params![source])
        .map_err(sqlite_error)?;
    let tx = conn.transaction().map_err(sqlite_error)?;
    for index in 0..ids.len() {
        let blob = floats_to_bytes(&embeddings[index]);
        let metadata = serde_json::to_string(&metadatas[index]).unwrap_or_else(|_| "{}".into());
        tx.execute(
            "INSERT OR REPLACE INTO chunks (id, source, document, metadata, embedding) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![ids[index], source, documents[index], metadata, blob],
        )
        .map_err(sqlite_error)?;
    }
    tx.commit().map_err(sqlite_error)?;
    Ok(())
}

fn local_query(inner: &VectorInner, query: &[f32], n: usize) -> Result<Vec<Hit>, VectorStoreError> {
    open_local(inner)?;
    let guard = inner.local.lock().unwrap_or_else(|err| err.into_inner());
    let conn = guard.as_ref().ok_or_else(|| {
        VectorStoreError::new(
            "Índice local no abierto.",
            "No se pudo conectar con la base vectorial. Verifica que chroma_db/ sea accesible.",
        )
    })?;
    let mut statement = conn
        .prepare("SELECT id, document, metadata, embedding FROM chunks")
        .map_err(sqlite_error)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Vec<u8>>(3)?,
            ))
        })
        .map_err(sqlite_error)?;
    let mut scored = Vec::new();
    for row in rows {
        let (id, document, metadata_raw, blob) = row.map_err(sqlite_error)?;
        let embedding = bytes_to_floats(&blob);
        if embedding.len() != query.len() {
            return Err(VectorStoreError::new(
                "La dimensión de los embeddings no coincide con la consulta.",
                "La base de conocimiento fue indexada con otro modelo. Vuelve a indexar desde la pestaña Base.",
            ));
        }
        let distance = cosine_distance(query, &embedding);
        let metadata =
            serde_json::from_str::<Map<String, Value>>(&metadata_raw).unwrap_or_default();
        scored.push((distance, id, document, metadata));
    }
    scored.sort_by(|left, right| {
        left.0
            .partial_cmp(&right.0)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    scored.truncate(n);
    Ok(scored
        .into_iter()
        .map(|(distance, id, document, metadata)| Hit {
            id: Some(id),
            document,
            metadata,
            distance: Some(distance),
        })
        .collect())
}

fn cosine_distance(left: &[f32], right: &[f32]) -> f64 {
    let mut dot = 0.0f64;
    let mut left_norm = 0.0f64;
    let mut right_norm = 0.0f64;
    for (a, b) in left.iter().zip(right.iter()) {
        let a = *a as f64;
        let b = *b as f64;
        dot += a * b;
        left_norm += a * a;
        right_norm += b * b;
    }
    if left_norm == 0.0 || right_norm == 0.0 {
        return 1.0;
    }
    let similarity = dot / (left_norm.sqrt() * right_norm.sqrt());
    1.0 - similarity
}

fn floats_to_bytes(values: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(values.len() * 4);
    for value in values {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes
}

fn bytes_to_floats(bytes: &[u8]) -> Vec<f32> {
    let (chunks, _) = bytes.as_chunks::<4>();
    chunks
        .iter()
        .map(|chunk| f32::from_le_bytes(*chunk))
        .collect()
}

fn sqlite_error(err: rusqlite::Error) -> VectorStoreError {
    VectorStoreError::new(
        format!("Error de SQLite: {err}"),
        "No se pudieron guardar los fragmentos en la base vectorial.",
    )
}

fn normalized_meta(path: &Path) -> Value {
    let meta = read_meta(path);
    let mut sources = meta
        .get("sources")
        .and_then(|value| value.as_object())
        .cloned()
        .unwrap_or_default();
    if let Some(source) = meta.get("source").and_then(|value| value.as_str()) {
        if !sources.contains_key(source) {
            sources.insert(
                source.to_string(),
                json!({
                    "sha256": meta.get("sha256").cloned().unwrap_or(Value::Null),
                    "chunks": meta.get("chunks").and_then(|value| value.as_i64()).unwrap_or(0),
                    "indexed_at": meta.get("indexed_at").and_then(|value| value.as_str()).unwrap_or(""),
                }),
            );
        }
    }
    let mut object = meta.as_object().cloned().unwrap_or_default();
    object.insert("sources".into(), Value::Object(sources));
    Value::Object(object)
}

fn read_meta(path: &Path) -> Value {
    let Ok(text) = fs::read_to_string(path) else {
        return json!({});
    };
    serde_json::from_str(&text).unwrap_or_else(|_| json!({}))
}

fn chroma_safe_metadata(metadata: &Map<String, Value>) -> Map<String, Value> {
    let mut safe = Map::new();
    for (key, value) in metadata {
        match value {
            Value::Null => {}
            Value::String(_) | Value::Number(_) | Value::Bool(_) => {
                safe.insert(key.clone(), value.clone());
            }
            other => {
                safe.insert(key.clone(), Value::String(other.to_string()));
            }
        }
    }
    safe
}

pub fn meta_i64(metadata: &Map<String, Value>, key: &str) -> Option<i64> {
    match metadata.get(key)? {
        Value::Number(number) => number
            .as_i64()
            .or_else(|| number.as_f64().map(|value| value as i64)),
        Value::String(text) => text.parse().ok(),
        _ => None,
    }
}

pub fn meta_string(metadata: &Map<String, Value>, key: &str) -> Option<String> {
    match metadata.get(key)? {
        Value::String(text) if !text.is_empty() => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::documents::DocumentService;
    use crate::embed::HashEmbedder;

    fn sample_service(dir: &Path) -> (DocumentService, VectorService) {
        let path = dir.join("documento.txt");
        fs::write(
            &path,
            "La política de la empresa indica que el soporte técnico responde en horario continuo.\n\n\
El código de cliente tiene el formato CLI-XXXXXX y se consulta en Mi cuenta.\n\n\
Para acceder a cPanel se debe entrar a Mis servicios y pulsar Acceder a cPanel.\n",
        )
        .unwrap();
        let documents = DocumentService {
            document_path: path,
            documents_directory: dir.to_path_buf(),
            chunk_size_min: 800,
            chunk_size_max: 1200,
            chunk_overlap: 200,
        };
        let mut settings = Settings::for_tests(dir);
        settings.chroma_persist_directory = dir.join("chroma_db");
        settings.index_meta_path = dir.join("chroma_db").join(".index_meta.json");
        let vector = VectorService::new(&settings, Arc::new(HashEmbedder { dimensions: 32 }));
        (documents, vector)
    }

    #[tokio::test]
    async fn stores_information_without_duplicating() {
        let dir = tempfile::tempdir().unwrap();
        let (documents, vector) = sample_service(dir.path());
        vector.connect().await.unwrap();
        let (chunks, _chars, fingerprint) = documents.process().unwrap();
        let stored = vector
            .index_chunks(&chunks, &fingerprint, true, false)
            .await
            .unwrap();
        assert_eq!(stored, chunks.len());
        assert_eq!(vector.count().await, chunks.len());
        let stored_again = vector
            .index_chunks(&chunks, &fingerprint, false, false)
            .await
            .unwrap();
        assert_eq!(stored_again, 0);
        assert_eq!(vector.count().await, chunks.len());
    }

    #[tokio::test]
    async fn query_returns_results() {
        let dir = tempfile::tempdir().unwrap();
        let (documents, vector) = sample_service(dir.path());
        vector.connect().await.unwrap();
        let (chunks, _chars, fingerprint) = documents.process().unwrap();
        vector
            .index_chunks(&chunks, &fingerprint, true, false)
            .await
            .unwrap();
        let hits = vector
            .query("¿Dónde veo el código de cliente?", 3)
            .await
            .unwrap();
        assert!(!hits.is_empty());
        assert!(hits.iter().all(|hit| hit.distance.is_some()));
        let joined = hits
            .iter()
            .map(|hit| hit.document.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        let lowered = joined.to_lowercase();
        assert!(
            joined.contains("CLI-XXXXXX")
                || joined.contains("cPanel")
                || lowered.contains("soporte")
        );
    }
}
