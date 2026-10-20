//! Cliente HTTP de la API v2 de ChromaDB (`reqwest`).

use std::time::Duration;

use serde_json::{json, Map, Value};

use crate::error::AppError;
use crate::vector::Hit;

#[derive(Debug, Clone)]
pub struct ChromaHttp {
    http: reqwest::Client,
    base: String,
    tenant: String,
    database: String,
    collection_name: String,
    collection_id: Option<String>,
}

impl ChromaHttp {
    pub fn new(
        base: &str,
        tenant: &str,
        database: &str,
        collection_name: &str,
    ) -> Result<Self, AppError> {
        crate::tls::install_ring_provider();
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent("micomco-rag/1.0")
            .build()
            .map_err(|err| {
                AppError::vector(
                    format!("No se pudo crear el cliente de Chroma: {err}"),
                    "No se pudo conectar con la base vectorial. Verifica que chroma_db/ sea accesible.",
                )
            })?;
        Ok(Self {
            http,
            base: base.trim_end_matches('/').to_string(),
            tenant: tenant.to_string(),
            database: database.to_string(),
            collection_name: collection_name.to_string(),
            collection_id: None,
        })
    }

    pub async fn heartbeat(&self) -> Result<(), AppError> {
        let url = format!("{}/api/v2/heartbeat", self.base);
        let response = self.http.get(url).send().await.map_err(connect_error)?;
        check(response, "connect").await.map(|_| ())
    }

    pub async fn count(&mut self) -> Result<usize, AppError> {
        let Some(id) = self.ensure_id(false).await? else {
            return Ok(0);
        };
        let url = format!("{}/count", self.collection_url(&id));
        let response = self.http.get(url).send().await.map_err(connect_error)?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            self.collection_id = None;
            return Ok(0);
        }
        let response = check(response, "count").await?;
        let bytes = read_body(response).await?;
        if bytes.is_empty() {
            return Ok(0);
        }
        let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        Ok(parse_count(&value))
    }

    pub async fn delete_collection(&mut self) -> Result<(), AppError> {
        let url = format!(
            "{}/api/v2/tenants/{}/databases/{}/collections/{}",
            self.base,
            self.tenant,
            self.database,
            urlencoding_path(&self.collection_name)
        );
        let response = self.http.delete(url).send().await.map_err(connect_error)?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            self.collection_id = None;
            return Ok(());
        }
        check(response, "reset").await?;
        self.collection_id = None;
        Ok(())
    }

    pub async fn delete_source(&mut self, source: &str) -> Result<(), AppError> {
        let Some(id) = self.ensure_id(true).await? else {
            return Ok(());
        };
        let url = format!("{}/get", self.collection_url(&id));
        let body = json!({
            "where": { "source": { "$eq": source } },
            "include": ["metadatas"]
        });
        let response = self
            .http
            .post(url)
            .json(&body)
            .send()
            .await
            .map_err(connect_error)?;
        if !response.status().is_success() {
            tracing::info!(%source, status = %response.status(), "no había chunks previos que eliminar");
            return Ok(());
        }
        let payload = response.json::<Value>().await.unwrap_or(Value::Null);
        let ids = payload
            .get("ids")
            .and_then(|value| value.as_array())
            .cloned()
            .unwrap_or_default();
        let ids: Vec<String> = ids
            .into_iter()
            .filter_map(|value| value.as_str().map(str::to_string))
            .collect();
        if ids.is_empty() {
            return Ok(());
        }
        let removed = ids.len();
        let delete_url = format!("{}/delete", self.collection_url(&id));
        let response = self
            .http
            .post(delete_url)
            .json(&json!({ "ids": ids }))
            .send()
            .await
            .map_err(connect_error)?;
        check(response, "index").await?;
        tracing::info!(removed, %source, "chunks previos eliminados en Chroma");
        Ok(())
    }

    pub async fn upsert(
        &mut self,
        ids: &[String],
        embeddings: &[Vec<f32>],
        documents: &[String],
        metadatas: &[Map<String, Value>],
    ) -> Result<(), AppError> {
        let Some(id) = self.ensure_id(true).await? else {
            return Err(AppError::vector(
                "Chroma no devolvió una colección.",
                "La base de conocimiento no está disponible. Ejecuta la indexación e inténtalo de nuevo.",
            ));
        };
        let url = format!("{}/upsert", self.collection_url(&id));
        let body = json!({
            "ids": ids,
            "embeddings": embeddings,
            "documents": documents,
            "metadatas": metadatas,
        });
        let response = self
            .http
            .post(url)
            .json(&body)
            .send()
            .await
            .map_err(connect_error)?;
        check(response, "index").await.map(|_| ())
    }

    pub async fn query(
        &mut self,
        embedding: &[f32],
        n_results: usize,
    ) -> Result<Vec<Hit>, AppError> {
        let Some(id) = self.ensure_id(false).await? else {
            return Ok(Vec::new());
        };
        let url = format!("{}/query", self.collection_url(&id));
        let body = json!({
            "query_embeddings": [embedding],
            "n_results": n_results,
            "include": ["documents", "metadatas", "distances"],
        });
        let response = self
            .http
            .post(url)
            .json(&body)
            .send()
            .await
            .map_err(connect_error)?;
        let response = check(response, "query").await?;
        let payload = response.json::<Value>().await.map_err(|err| {
            AppError::vector(
                format!("Respuesta de consulta ilegible: {err}"),
                "No se pudo buscar en la base de conocimiento. Inténtalo de nuevo.",
            )
        })?;
        Ok(parse_query(&payload))
    }

    async fn ensure_id(&mut self, create: bool) -> Result<Option<String>, AppError> {
        if let Some(id) = &self.collection_id {
            return Ok(Some(id.clone()));
        }
        if let Some(id) = self.find_collection_id().await? {
            self.collection_id = Some(id.clone());
            return Ok(Some(id));
        }
        if !create {
            return Ok(None);
        }
        let url = format!(
            "{}/api/v2/tenants/{}/databases/{}/collections",
            self.base, self.tenant, self.database
        );
        let body = json!({
            "name": self.collection_name,
            "metadata": { "hnsw:space": "cosine" },
            "get_or_create": true,
        });
        let response = self
            .http
            .post(url)
            .json(&body)
            .send()
            .await
            .map_err(connect_error)?;
        let response = check(response, "collection").await?;
        let payload = response.json::<Value>().await.map_err(|err| {
            AppError::vector(
                format!("No se pudo leer la colección: {err}"),
                "La base de conocimiento no está disponible. Ejecuta la indexación e inténtalo de nuevo.",
            )
        })?;
        let id = payload
            .get("id")
            .and_then(|value| value.as_str())
            .ok_or_else(|| {
                AppError::vector(
                    "La colección de Chroma no trae id.",
                    "La base de conocimiento no está disponible. Ejecuta la indexación e inténtalo de nuevo.",
                )
            })?
            .to_string();
        self.collection_id = Some(id.clone());
        Ok(Some(id))
    }

    async fn find_collection_id(&self) -> Result<Option<String>, AppError> {
        let url = format!(
            "{}/api/v2/tenants/{}/databases/{}/collections",
            self.base, self.tenant, self.database
        );
        let response = self.http.get(url).send().await.map_err(connect_error)?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let response = check(response, "collection").await?;
        let payload = response.json::<Value>().await.unwrap_or(Value::Null);
        let list = payload
            .as_array()
            .cloned()
            .or_else(|| {
                payload
                    .get("collections")
                    .and_then(|value| value.as_array())
                    .cloned()
            })
            .unwrap_or_default();
        for item in list {
            if item.get("name").and_then(|value| value.as_str())
                == Some(self.collection_name.as_str())
            {
                if let Some(id) = item.get("id").and_then(|value| value.as_str()) {
                    return Ok(Some(id.to_string()));
                }
            }
        }
        Ok(None)
    }

    fn collection_url(&self, id: &str) -> String {
        format!(
            "{}/api/v2/tenants/{}/databases/{}/collections/{}",
            self.base,
            self.tenant,
            self.database,
            urlencoding_path(id)
        )
    }
}

fn parse_count(value: &Value) -> usize {
    if let Some(number) = value.as_u64() {
        return number as usize;
    }
    if let Some(number) = value.as_i64() {
        return number.max(0) as usize;
    }
    value
        .get("count")
        .and_then(|item| {
            item.as_u64()
                .or_else(|| item.as_i64().map(|n| n.max(0) as u64))
        })
        .unwrap_or(0) as usize
}

fn parse_query(payload: &Value) -> Vec<Hit> {
    let documents = first_batch(payload.get("documents"));
    let metadatas = first_batch(payload.get("metadatas"));
    let distances = first_batch(payload.get("distances"));
    let ids = first_batch(payload.get("ids"));
    let mut hits = Vec::new();
    for (index, document) in documents.iter().enumerate() {
        let Some(document) = document.as_str() else {
            continue;
        };
        let metadata = metadatas
            .get(index)
            .and_then(|value| value.as_object())
            .cloned()
            .unwrap_or_default();
        let distance = distances.get(index).and_then(|value| value.as_f64());
        let id = ids
            .get(index)
            .and_then(|value| value.as_str())
            .map(str::to_string);
        hits.push(Hit {
            id,
            document: document.to_string(),
            metadata,
            distance,
        });
    }
    hits
}

fn first_batch(value: Option<&Value>) -> Vec<Value> {
    value
        .and_then(|item| item.as_array())
        .and_then(|batch| batch.first())
        .and_then(|row| row.as_array())
        .cloned()
        .unwrap_or_default()
}

async fn check(response: reqwest::Response, action: &str) -> Result<reqwest::Response, AppError> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let body = response.text().await.unwrap_or_default();
    Err(chroma_error(action, status, &body))
}

fn chroma_error(action: &str, status: reqwest::StatusCode, body: &str) -> AppError {
    let user_message = match action {
        "index" => "No se pudieron guardar los fragmentos en la base vectorial.",
        "query" => "No se pudo buscar en la base de conocimiento. Inténtalo de nuevo.",
        "connect" => {
            "No se pudo conectar con la base vectorial. Verifica que el servidor Chroma esté accesible."
        }
        _ => "La base de conocimiento no está disponible. Ejecuta la indexación e inténtalo de nuevo.",
    };
    AppError::vector(
        format!("Chroma HTTP {status} durante {action}: {body}"),
        user_message,
    )
}

fn connect_error(err: reqwest::Error) -> AppError {
    AppError::vector(
        format!("Error al conectar con ChromaDB: {err}"),
        "No se pudo conectar con la base vectorial. Verifica que el servidor Chroma esté accesible.",
    )
}

async fn read_body(response: reqwest::Response) -> Result<Vec<u8>, AppError> {
    use futures_util::StreamExt;
    let mut stream = response.bytes_stream();
    let mut buffer = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|err| {
            AppError::vector(
                format!("Lectura de Chroma interrumpida: {err}"),
                "No se pudo consultar la base de conocimiento. Inténtalo de nuevo en unos minutos.",
            )
        })?;
        buffer.extend_from_slice(&chunk);
    }
    Ok(buffer)
}

fn urlencoding_path(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        let ch = byte as char;
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | '~') {
            encoded.push(ch);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::ChromaHttp;
    use axum::extract::State;
    use axum::routing::{delete, get, post};
    use axum::{Json, Router};
    use serde_json::{json, Value};
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct MockDb {
        collection_id: Option<String>,
        records: Vec<Stored>,
    }

    struct Stored {
        id: String,
        document: String,
        metadata: serde_json::Map<String, Value>,
        embedding: Vec<f32>,
    }

    #[tokio::test]
    async fn client_roundtrip_against_v2_api() {
        let db = Arc::new(Mutex::new(MockDb::default()));
        let app = Router::new()
            .route(
                "/api/v2/heartbeat",
                get(|| async { Json(json!({"nanosecond heartbeat": 1})) }),
            )
            .route(
                "/api/v2/tenants/default_tenant/databases/default_database/collections",
                get(list_collections).post(create_collection),
            )
            .route(
                "/api/v2/tenants/default_tenant/databases/default_database/collections/:name",
                delete(delete_collection),
            )
            .route(
                "/api/v2/tenants/default_tenant/databases/default_database/collections/:id/count",
                get(count),
            )
            .route(
                "/api/v2/tenants/default_tenant/databases/default_database/collections/:id/upsert",
                post(upsert),
            )
            .route(
                "/api/v2/tenants/default_tenant/databases/default_database/collections/:id/get",
                post(get_records),
            )
            .route(
                "/api/v2/tenants/default_tenant/databases/default_database/collections/:id/delete",
                post(delete_records),
            )
            .route(
                "/api/v2/tenants/default_tenant/databases/default_database/collections/:id/query",
                post(query),
            )
            .with_state(db);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let mut client = ChromaHttp::new(
            &format!("http://{address}"),
            "default_tenant",
            "default_database",
            "empresa_knowledge_base",
        )
        .unwrap();
        client.heartbeat().await.unwrap();
        assert_eq!(client.count().await.unwrap(), 0);
        client
            .upsert(
                &["documento.txt::1".to_string()],
                &[vec![1.0, 0.0, 0.0]],
                &["El código de cliente es CLI-XXXXXX.".to_string()],
                &[serde_json::Map::from_iter([(
                    "source".to_string(),
                    Value::String("documento.txt".to_string()),
                )])],
            )
            .await
            .unwrap();
        assert_eq!(client.count().await.unwrap(), 1);
        let hits = client.query(&[1.0, 0.0, 0.0], 1).await.unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].document.contains("CLI-XXXXXX"));
        client.delete_source("documento.txt").await.unwrap();
        assert_eq!(client.count().await.unwrap(), 0);
        client.delete_collection().await.unwrap();
        assert_eq!(client.count().await.unwrap(), 0);
    }

    async fn list_collections(State(db): State<Arc<Mutex<MockDb>>>) -> Json<Value> {
        let db = db.lock().unwrap();
        if let Some(id) = &db.collection_id {
            Json(json!([{ "id": id, "name": "empresa_knowledge_base" }]))
        } else {
            Json(json!([]))
        }
    }

    async fn create_collection(State(db): State<Arc<Mutex<MockDb>>>) -> Json<Value> {
        let mut db = db.lock().unwrap();
        if db.collection_id.is_none() {
            db.collection_id = Some("col-1".to_string());
        }
        Json(json!({ "id": db.collection_id, "name": "empresa_knowledge_base" }))
    }

    async fn delete_collection(State(db): State<Arc<Mutex<MockDb>>>) -> StatusCode {
        let mut db = db.lock().unwrap();
        db.collection_id = None;
        db.records.clear();
        StatusCode::OK
    }

    async fn count(State(db): State<Arc<Mutex<MockDb>>>) -> Json<usize> {
        Json(db.lock().unwrap().records.len())
    }

    async fn upsert(State(db): State<Arc<Mutex<MockDb>>>, Json(body): Json<Value>) -> StatusCode {
        let mut db = db.lock().unwrap();
        let ids = body
            .get("ids")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let docs = body
            .get("documents")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let metas = body
            .get("metadatas")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let embs = body
            .get("embeddings")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        for (index, id_value) in ids.iter().enumerate() {
            let id = id_value.as_str().unwrap_or_default().to_string();
            let document = docs
                .get(index)
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let metadata = metas
                .get(index)
                .and_then(|v| v.as_object())
                .cloned()
                .unwrap_or_default();
            let embedding = embs
                .get(index)
                .and_then(|v| v.as_array())
                .map(|row| {
                    row.iter()
                        .filter_map(|n| n.as_f64().map(|n| n as f32))
                        .collect()
                })
                .unwrap_or_default();
            if let Some(existing) = db.records.iter_mut().find(|record| record.id == id) {
                existing.document = document;
                existing.metadata = metadata;
                existing.embedding = embedding;
            } else {
                db.records.push(Stored {
                    id,
                    document,
                    metadata,
                    embedding,
                });
            }
        }
        StatusCode::OK
    }

    async fn get_records(
        State(db): State<Arc<Mutex<MockDb>>>,
        Json(body): Json<Value>,
    ) -> Json<Value> {
        let source = body
            .pointer("/where/source/$eq")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let db = db.lock().unwrap();
        let ids: Vec<String> = db
            .records
            .iter()
            .filter(|record| record.metadata.get("source").and_then(|v| v.as_str()) == Some(source))
            .map(|record| record.id.clone())
            .collect();
        Json(json!({ "ids": ids }))
    }

    async fn delete_records(
        State(db): State<Arc<Mutex<MockDb>>>,
        Json(body): Json<Value>,
    ) -> StatusCode {
        let ids = body
            .get("ids")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let mut db = db.lock().unwrap();
        db.records
            .retain(|record| !ids.iter().any(|id| id.as_str() == Some(record.id.as_str())));
        StatusCode::OK
    }

    async fn query(State(db): State<Arc<Mutex<MockDb>>>) -> Json<Value> {
        let db = db.lock().unwrap();
        let ids: Vec<&str> = db.records.iter().map(|record| record.id.as_str()).collect();
        let documents: Vec<&str> = db
            .records
            .iter()
            .map(|record| record.document.as_str())
            .collect();
        let metadatas: Vec<&serde_json::Map<String, Value>> =
            db.records.iter().map(|record| &record.metadata).collect();
        let distances = vec![0.1_f64; db.records.len()];
        Json(json!({
            "ids": [ids],
            "documents": [documents],
            "metadatas": [metadatas],
            "distances": [distances],
        }))
    }

    use axum::http::StatusCode;
}
