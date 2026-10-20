//! Embeddings locales. Producción usa all-MiniLM-L6-v2 (fastembed).
//! Las pruebas inyectan `HashEmbedder`, el mismo esquema determinista del FakeEmbeddingFunction.

use std::sync::Arc;

use async_trait::async_trait;
use sha2::{Digest, Sha256};

use crate::config::Settings;
use crate::vector::VectorStoreError;

#[async_trait]
pub trait Embedder: Send + Sync {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, VectorStoreError>;
}

/// Embeddings deterministas para pruebas. No descarga modelos.
pub struct HashEmbedder {
    pub dimensions: usize,
}

#[async_trait]
impl Embedder for HashEmbedder {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, VectorStoreError> {
        let mut vectors = Vec::with_capacity(texts.len());
        for text in texts {
            let mut hasher = Sha256::new();
            hasher.update(text.as_bytes());
            let digest: [u8; 32] = hasher.finalize().into();
            let mut values = Vec::with_capacity(self.dimensions);
            for index in 0..self.dimensions {
                values.push(digest[index % digest.len()] as f32 / 255.0);
            }
            if !values.is_empty() {
                let length = text.chars().count();
                values[0] = (values[0] + (length % 50) as f32 / 50.0) / 2.0;
            }
            vectors.push(values);
        }
        Ok(vectors)
    }
}

#[cfg(feature = "semantic-embeddings")]
mod fast {
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use fastembed::{EmbeddingModel, TextEmbedding, TextInitOptions};

    use super::Embedder;
    use crate::config::Settings;
    use crate::vector::VectorStoreError;

    pub struct FastEmbedder {
        model: EmbeddingModel,
        cache_dir: PathBuf,
        inner: Arc<Mutex<Option<TextEmbedding>>>,
    }

    impl FastEmbedder {
        pub fn new(settings: &Settings) -> Result<Self, VectorStoreError> {
            let model = match settings.embedding_model.trim() {
                "all-MiniLM-L6-v2" | "sentence-transformers/all-MiniLM-L6-v2" | "AllMiniLML6V2" => {
                    EmbeddingModel::AllMiniLML6V2
                }
                other => {
                    return Err(VectorStoreError::new(
                        format!("modelo no soportado: {other}"),
                        format!(
                            "El modelo de embeddings '{other}' no está soportado. Usa EMBEDDING_MODEL=all-MiniLM-L6-v2."
                        ),
                    ));
                }
            };
            Ok(Self {
                model,
                cache_dir: settings.fastembed_cache.clone(),
                inner: Arc::new(Mutex::new(None)),
            })
        }
    }

    #[async_trait]
    impl Embedder for FastEmbedder {
        async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, VectorStoreError> {
            if texts.is_empty() {
                return Ok(Vec::new());
            }
            let owned = texts.to_vec();
            let inner = Arc::clone(&self.inner);
            let model = self.model.clone();
            let cache_dir = self.cache_dir.clone();
            tokio::task::spawn_blocking(move || {
                let mut guard = inner.lock().unwrap_or_else(|err| err.into_inner());
                if guard.is_none() {
                    *guard = Some(load_model(model, cache_dir)?);
                }
                let engine = guard.as_mut().expect("el modelo se acaba de cargar");
                engine.embed(owned, None).map_err(|err| {
                    VectorStoreError::new(
                        format!("Error al generar embeddings: {err}"),
                        "No se pudieron generar los embeddings del documento. Revisa el modelo configurado.",
                    )
                })
            })
            .await
            .map_err(|err| {
                VectorStoreError::new(
                    format!("El hilo de embeddings falló: {err}"),
                    "No se pudieron generar los embeddings del documento. Revisa el modelo configurado.",
                )
            })?
        }
    }

    fn load_model(
        model: EmbeddingModel,
        cache_dir: PathBuf,
    ) -> Result<TextEmbedding, VectorStoreError> {
        std::fs::create_dir_all(&cache_dir).map_err(|err| {
            VectorStoreError::new(
                format!("No se pudo crear la caché de embeddings: {err}"),
                "No se pudieron generar los embeddings del documento. Revisa el modelo configurado.",
            )
        })?;
        tracing::info!("Inicializando modelo de embeddings all-MiniLM-L6-v2");
        TextEmbedding::try_new(
            TextInitOptions::new(model)
                .with_cache_dir(cache_dir)
                .with_show_download_progress(true),
        )
        .map_err(|err| {
            VectorStoreError::new(
                format!("Error al inicializar embeddings: {err}"),
                "No se pudieron generar los embeddings del documento. Revisa el modelo configurado.",
            )
        })
    }
}

#[cfg(feature = "semantic-embeddings")]
pub use fast::FastEmbedder;

pub fn production_embedder(settings: &Settings) -> Result<Arc<dyn Embedder>, VectorStoreError> {
    #[cfg(feature = "semantic-embeddings")]
    {
        Ok(Arc::new(FastEmbedder::new(settings)?))
    }
    #[cfg(not(feature = "semantic-embeddings"))]
    {
        let _ = settings;
        Err(VectorStoreError::new(
            "El binario se compiló sin el feature semantic-embeddings.",
            "Recompila el servidor con embeddings semánticos (fastembed / all-MiniLM-L6-v2).",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn hash_embedder_shape() {
        let embedder = HashEmbedder { dimensions: 16 };
        let vectors = embedder
            .embed(&["hola".into(), "mundo".into()])
            .await
            .unwrap();
        assert_eq!(vectors.len(), 2);
        assert_eq!(vectors[0].len(), 16);
        assert_ne!(vectors[0], vectors[1]);
    }
}
