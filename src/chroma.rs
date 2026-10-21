//! Cliente HTTP de la API v2 de ChromaDB (`reqwest`, cuerpo leído en streaming).

use std::sync::Mutex;
use std::time::Duration;

use futures_util::StreamExt;
use reqwest::header::{HeaderMap, HeaderValue};
use reqwest::{Client, StatusCode};
use serde_json::{json, Map, Value};

use crate::models::Hit;
use crate::vector::VectorStoreError;

const MAX_BODY: usize = 32 * 1024 * 1024;

#[derive(Clone)]
pub struct ChromaClient {
    http: Client,
    base: String,
    tenant: String,
    database: String,
    collection_name: String,
    collection_id: std::sync::Arc<Mutex<Option<String>>>,
}

impl ChromaClient {
    pub fn new(
        base_url: &str,
        tenant: &str,
        database: &str,
        collection_name: &str,
        token: Option<&str>,
    ) -> Result<Self, VectorStoreError> {
        let mut headers = HeaderMap::new();
        if let Some(token) = token {
            let value = HeaderValue::from_str(token).map_err(|err| {
                VectorStoreError::new(
                    format!("CHROMA_TOKEN inválido: {err}"),
                    "El token de ChromaDB no es válido. Revisa CHROMA_TOKEN.",
                )
            })?;
            headers.insert("x-chroma-token", value);
        }
        let http = Client::builder()
            .timeout(Duration::from_secs(90))
            .default_headers(headers)
            .build()
            .map_err(|err| {
                VectorStoreError::new(
                    format!("No se pudo crear el cliente HTTP de Chroma: {err}"),
                    "No se pudo conectar con la base vectorial.",
                )
            })?;
        Ok(Self {
            http,
            base: base_url.trim().trim_end_matches('/').to_string(),
            tenant: tenant.to_string(),
            database: database.to_string(),
            collection_name: collection_name.to_string(),
            collection_id: std::sync::Arc::new(Mutex::new(None)),
        })
    }

    pub async fn heartbeat(&self) -> Result<(), VectorStoreError> {
        let url = format!("{}/api/v2/heartbeat", self.base);
        let (status, _) = self.request(self.http.get(url)).await?;
        if status.is_success() {
            Ok(())
        } else {
            Err(VectorStoreError::new(
                format!("Heartbeat de ChromaDB respondió {status}"),
                "No se pudo conectar con la base vectorial. Verifica que chroma_db/ sea accesible.",
            ))
        }
    }

    pub async fn count(&self) -> Result<usize, VectorStoreError> {
        let Some(id) = self.lookup_collection_id().await? else {
            return Ok(0);
        };
        let url = self.collection_url(&id, "/count");
        let (status, body) = self.request(self.http.get(url)).await?;
        if status == StatusCode::NOT_FOUND {
            return Ok(0);
        }
        if !status.is_success() {
            return Err(self.http_error("contar", status, &body));
        }
        Ok(parse_count(&body))
    }

    pub async fn reset_collection(&self) -> Result<(), VectorStoreError> {
        let url = format!(
            "{}/api/v2/tenants/{}/databases/{}/collections/{}",
            self.base,
            path_segment(&self.tenant),
            path_segment(&self.database),
            path_segment(&self.collection_name)
        );
        let (status, body) = self.request(self.http.delete(url)).await?;
        if !(status.is_success() || status == StatusCode::NOT_FOUND) {
            return Err(self.http_error("borrar la colección", status, &body));
        }
        *self
            .collection_id
            .lock()
            .unwrap_or_else(|err| err.into_inner()) = None;
        Ok(())
    }

    pub async fn delete_source(&self, source: &str) -> Result<(), VectorStoreError> {
        let id = self.ensure_collection_id().await?;
        let url = self.collection_url(&id, "/get");
        let payload = json!({
            "where": {"source": source},
            "include": ["metadatas"]
        });
        let (status, body) = self.request(self.http.post(url).json(&payload)).await?;
        if status == StatusCode::NOT_FOUND {
            return Ok(());
        }
        if !status.is_success() {
            tracing::info!("No había chunks previos que eliminar para '{source}'.");
            return Ok(());
        }
        let ids = body
            .get("ids")
            .and_then(|value| value.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str().map(str::to_string))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if ids.is_empty() {
            return Ok(());
        }
        let delete_url = self.collection_url(&id, "/delete");
        let (status, body) = self
            .request(self.http.post(delete_url).json(&json!({ "ids": ids })))
            .await?;
        if !status.is_success() {
            return Err(self.http_error("eliminar fragmentos previos", status, &body));
        }
        tracing::info!("Se eliminaron {} chunks previos de '{source}'.", ids.len());
        Ok(())
    }

    pub async fn upsert(
        &self,
        ids: &[String],
        documents: &[String],
        metadatas: &[Map<String, Value>],
        embeddings: &[Vec<f32>],
    ) -> Result<(), VectorStoreError> {
        let id = self.ensure_collection_id().await?;
        let url = self.collection_url(&id, "/upsert");
        let batch = 64;
        for start in (0..ids.len()).step_by(batch) {
            let end = (start + batch).min(ids.len());
            let payload = json!({
                "ids": &ids[start..end],
                "documents": &documents[start..end],
                "metadatas": &metadatas[start..end],
                "embeddings": &embeddings[start..end],
            });
            let (status, body) = self
                .request(self.http.post(url.clone()).json(&payload))
                .await?;
            if !status.is_success() {
                return Err(self.http_error("indexar", status, &body));
            }
        }
        Ok(())
    }

    pub async fn query(
        &self,
        embedding: &[f32],
        n_results: usize,
    ) -> Result<Vec<Hit>, VectorStoreError> {
        let id = self.ensure_collection_id().await?;
        let url = self.collection_url(&id, "/query");
        let payload = json!({
            "query_embeddings": [embedding],
            "n_results": n_results.max(1),
            "include": ["documents", "metadatas", "distances"]
        });
        let (status, body) = self.request(self.http.post(url).json(&payload)).await?;
        if !status.is_success() {
            return Err(self.http_error("consultar", status, &body));
        }
        Ok(hits_from_query_response(&body))
    }

    async fn ensure_collection_id(&self) -> Result<String, VectorStoreError> {
        if let Some(id) = self.cached_id() {
            return Ok(id);
        }
        if let Some(id) = self.lookup_collection_id().await? {
            return Ok(id);
        }
        let url = format!(
            "{}/api/v2/tenants/{}/databases/{}/collections",
            self.base,
            path_segment(&self.tenant),
            path_segment(&self.database)
        );
        let payload = json!({
            "name": self.collection_name,
            "metadata": {"hnsw:space": "cosine"},
            "get_or_create": true
        });
        let (status, body) = self.request(self.http.post(url).json(&payload)).await?;
        if status == StatusCode::CONFLICT {
            if let Some(id) = self.lookup_collection_id().await? {
                return Ok(id);
            }
        }
        if !status.is_success() {
            return Err(VectorStoreError::new(
                format!("No se pudo crear la colección (HTTP {status}): {body}"),
                "La base de conocimiento no está disponible. Ejecuta la indexación e inténtalo de nuevo.",
            ));
        }
        let id = collection_id_from_body(&body).ok_or_else(|| {
            VectorStoreError::new(
                "ChromaDB no devolvió el id de la colección.",
                "La base de conocimiento no está disponible. Ejecuta la indexación e inténtalo de nuevo.",
            )
        })?;
        *self
            .collection_id
            .lock()
            .unwrap_or_else(|err| err.into_inner()) = Some(id.clone());
        Ok(id)
    }

    async fn lookup_collection_id(&self) -> Result<Option<String>, VectorStoreError> {
        if let Some(id) = self.cached_id() {
            return Ok(Some(id));
        }
        let url = format!(
            "{}/api/v2/tenants/{}/databases/{}/collections/{}",
            self.base,
            path_segment(&self.tenant),
            path_segment(&self.database),
            path_segment(&self.collection_name)
        );
        let (status, body) = self.request(self.http.get(url)).await?;
        if status == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !status.is_success() {
            return Err(self.http_error("abrir la colección", status, &body));
        }
        let Some(id) = collection_id_from_body(&body) else {
            return Ok(None);
        };
        *self
            .collection_id
            .lock()
            .unwrap_or_else(|err| err.into_inner()) = Some(id.clone());
        Ok(Some(id))
    }

    fn cached_id(&self) -> Option<String> {
        self.collection_id
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .clone()
    }

    fn collection_url(&self, id: &str, suffix: &str) -> String {
        format!(
            "{}/api/v2/tenants/{}/databases/{}/collections/{}{suffix}",
            self.base,
            path_segment(&self.tenant),
            path_segment(&self.database),
            path_segment(id)
        )
    }

    async fn request(
        &self,
        builder: reqwest::RequestBuilder,
    ) -> Result<(StatusCode, Value), VectorStoreError> {
        let response = builder.send().await.map_err(|err| {
            VectorStoreError::new(
                format!("Error de red con ChromaDB: {err}"),
                "No se pudo conectar con la base vectorial. Verifica que chroma_db/ sea accesible.",
            )
        })?;
        let status = response.status();
        let bytes = read_body_stream(response).await?;
        if bytes.is_empty() {
            return Ok((status, Value::Null));
        }
        let value = serde_json::from_slice(&bytes).unwrap_or_else(|_| {
            Value::String(String::from_utf8_lossy(&bytes).chars().take(500).collect())
        });
        Ok((status, value))
    }

    fn http_error(&self, action: &str, status: StatusCode, body: &Value) -> VectorStoreError {
        VectorStoreError::new(
            format!("Error al {action} en ChromaDB (HTTP {status}): {body}"),
            "No se pudo consultar la base de conocimiento. Inténtalo de nuevo en unos minutos.",
        )
    }
}

async fn read_body_stream(response: reqwest::Response) -> Result<Vec<u8>, VectorStoreError> {
    let mut stream = response.bytes_stream();
    let mut buffer = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|err| {
            VectorStoreError::new(
                format!("Lectura incompleta de ChromaDB: {err}"),
                "No se pudo consultar la base de conocimiento. Inténtalo de nuevo en unos minutos.",
            )
        })?;
        if buffer.len().saturating_add(chunk.len()) > MAX_BODY {
            return Err(VectorStoreError::new(
                "La respuesta de ChromaDB supera el tamaño permitido.",
                "No se pudo consultar la base de conocimiento. Inténtalo de nuevo en unos minutos.",
            ));
        }
        buffer.extend_from_slice(&chunk);
    }
    Ok(buffer)
}

pub fn hits_from_query_response(body: &Value) -> Vec<Hit> {
    let documents = nested_array(body, "documents");
    let metadatas = nested_array(body, "metadatas");
    let distances = nested_array(body, "distances");
    let ids = nested_array(body, "ids");
    let mut hits = Vec::new();
    for (index, document) in documents.iter().enumerate() {
        let Some(text) = document.as_str() else {
            continue;
        };
        let metadata = metadatas
            .get(index)
            .and_then(|value| value.as_object())
            .cloned()
            .unwrap_or_default();
        let distance = distances.get(index).copied().and_then(value_f64);
        let id = ids
            .get(index)
            .and_then(|value| value.as_str())
            .map(str::to_string);
        hits.push(Hit {
            id,
            document: text.to_string(),
            metadata,
            distance,
        });
    }
    hits
}

fn nested_array<'a>(body: &'a Value, key: &str) -> Vec<&'a Value> {
    body.get(key)
        .and_then(|value| value.as_array())
        .and_then(|outer| outer.first())
        .and_then(|inner| inner.as_array())
        .map(|items| items.iter().collect())
        .unwrap_or_default()
}

fn parse_count(body: &Value) -> usize {
    if let Some(number) = body.as_u64() {
        return number as usize;
    }
    if let Some(number) = body.as_i64() {
        return number.max(0) as usize;
    }
    body.get("count")
        .or_else(|| body.get("value"))
        .and_then(|value| {
            value
                .as_u64()
                .or_else(|| value.as_i64().map(|n| n.max(0) as u64))
        })
        .unwrap_or(0) as usize
}

fn collection_id_from_body(body: &Value) -> Option<String> {
    body.get("id")
        .and_then(|value| value.as_str())
        .map(str::to_string)
        .or_else(|| {
            body.get("collection")
                .and_then(|value| value.get("id"))
                .and_then(|value| value.as_str())
                .map(str::to_string)
        })
}

fn value_f64(value: &Value) -> Option<f64> {
    value.as_f64().or_else(|| value.as_i64().map(|n| n as f64))
}

fn path_segment(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nested_query_payload() {
        let body = json!({
            "ids": [["documento.txt::1"]],
            "documents": [["El código CLI-XXXXXX está en Mi cuenta."]],
            "metadatas": [[{"source": "documento.txt", "chunk_id": 1, "title": "Código"}]],
            "distances": [[0.125]]
        });
        let hits = hits_from_query_response(&body);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id.as_deref(), Some("documento.txt::1"));
        assert!(hits[0].document.contains("CLI-XXXXXX"));
        assert_eq!(hits[0].distance, Some(0.125));
        assert_eq!(
            hits[0].metadata.get("title").and_then(|v| v.as_str()),
            Some("Código")
        );
    }

    #[test]
    fn parses_count_number_or_object() {
        assert_eq!(parse_count(&json!(12)), 12);
        assert_eq!(parse_count(&json!({"count": 4})), 4);
    }
}
