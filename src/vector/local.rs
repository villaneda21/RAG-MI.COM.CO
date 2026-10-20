//! Índice vectorial persistente en `chroma_db/local_vectors.json`.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::embeddings::{bm25_scores, dot};
use crate::error::AppError;
use crate::vector::Hit;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalRecord {
    pub id: String,
    pub document: String,
    pub metadata: Map<String, Value>,
    pub embedding: Vec<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct LocalIndexFile {
    records: Vec<LocalRecord>,
}

#[derive(Debug, Clone)]
pub struct LocalStore {
    pub path: PathBuf,
    pub records: Vec<LocalRecord>,
}

impl LocalStore {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            records: Vec::new(),
        }
    }

    pub fn load(&mut self) -> Result<(), AppError> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|err| {
                AppError::vector(
                    format!("Error al conectar con la base vectorial: {err}"),
                    "No se pudo conectar con la base vectorial. Verifica que chroma_db/ sea accesible.",
                )
            })?;
        }
        if !self.path.exists() {
            self.records.clear();
            return Ok(());
        }
        let text = std::fs::read_to_string(&self.path).map_err(|err| {
            AppError::vector(
                format!("No se pudo leer el índice local: {err}"),
                "No se pudo conectar con la base vectorial. Verifica que chroma_db/ sea accesible.",
            )
        })?;
        match serde_json::from_str::<LocalIndexFile>(&text) {
            Ok(file) => {
                self.records = file.records;
                Ok(())
            }
            Err(err) => {
                tracing::warn!(error = %err, "índice local ilegible; se reinicia vacío");
                self.records.clear();
                Ok(())
            }
        }
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    pub fn clear(&mut self) -> Result<(), AppError> {
        self.records.clear();
        self.persist()
    }

    pub fn delete_source(&mut self, source: &str) {
        let before = self.records.len();
        self.records.retain(|record| {
            record
                .metadata
                .get("source")
                .and_then(|value| value.as_str())
                != Some(source)
        });
        let removed = before.saturating_sub(self.records.len());
        if removed > 0 {
            tracing::info!(removed, %source, "chunks previos eliminados");
        }
    }

    pub fn upsert(&mut self, records: Vec<LocalRecord>) -> Result<(), AppError> {
        for record in records {
            if let Some(existing) = self.records.iter_mut().find(|item| item.id == record.id) {
                *existing = record;
            } else {
                self.records.push(record);
            }
        }
        self.persist()
    }

    pub fn query(&self, embedding: &[f32], question: &str, n_results: usize) -> Vec<Hit> {
        if self.records.is_empty() || n_results == 0 {
            return Vec::new();
        }
        let documents: Vec<String> = self
            .records
            .iter()
            .map(|record| record.document.clone())
            .collect();
        let bm25 = bm25_scores(question, &documents);
        let max_bm25 = bm25.iter().copied().fold(0.0_f32, f32::max).max(1.0e-6);
        let mut ranked: Vec<Hit> = self
            .records
            .iter()
            .zip(bm25)
            .map(|(record, bm25_score)| {
                let cosine = if record.embedding.len() == embedding.len() {
                    dot(&record.embedding, embedding)
                } else {
                    0.0
                };
                let cosine_unit = (cosine + 1.0) / 2.0;
                let bm25_unit = (bm25_score / max_bm25).clamp(0.0, 1.0);
                let blended = 0.55 * cosine_unit + 0.45 * bm25_unit;
                Hit {
                    id: Some(record.id.clone()),
                    document: record.document.clone(),
                    metadata: record.metadata.clone(),
                    distance: Some(f64::from(1.0 - blended)),
                }
            })
            .collect();
        ranked.sort_by(|left, right| {
            let left_distance = left.distance.unwrap_or(f64::MAX);
            let right_distance = right.distance.unwrap_or(f64::MAX);
            left_distance
                .partial_cmp(&right_distance)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| left.id.cmp(&right.id))
        });
        ranked.truncate(n_results);
        ranked
    }

    fn persist(&self) -> Result<(), AppError> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|err| {
                AppError::vector(
                    format!("No se pudo guardar el índice: {err}"),
                    "No se pudieron guardar los fragmentos en la base vectorial.",
                )
            })?;
        }
        let payload = serde_json::to_vec_pretty(&LocalIndexFile {
            records: self.records.clone(),
        })
        .map_err(|err| {
            AppError::vector(
                format!("No se pudo serializar el índice: {err}"),
                "No se pudieron guardar los fragmentos en la base vectorial.",
            )
        })?;
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, payload).map_err(|err| {
            AppError::vector(
                format!("No se pudo escribir el índice: {err}"),
                "No se pudieron guardar los fragmentos en la base vectorial.",
            )
        })?;
        std::fs::rename(&tmp, &self.path).map_err(|err| {
            AppError::vector(
                format!("No se pudo reemplazar el índice: {err}"),
                "No se pudieron guardar los fragmentos en la base vectorial.",
            )
        })
    }
}
