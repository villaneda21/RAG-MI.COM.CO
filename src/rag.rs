//! Orquestación RAG: recuperar guías, responder con Claude y guardar el turno.

use std::path::Path;

#[cfg(test)]
use crate::config::Settings;
use crate::documents::{fingerprint_text, DocumentService};
use crate::error::AppError;
use crate::models::{
    json_i64, json_str, ChatMessage, ChatResponse, IndexFileStatus, IndexStatusResponse,
    ReindexResponse,
};
use crate::state::AppState;
use crate::vector::Hit;

const CHAT_INTERNAL: &str =
    "Ocurrió un error interno al generar la respuesta. Revisa la consola del servidor.";

impl AppState {
    pub async fn ask(
        &self,
        question: String,
        history: Vec<ChatMessage>,
        session_id: Option<String>,
    ) -> Result<ChatResponse, AppError> {
        let cleaned = question.trim().to_string();
        if cleaned.is_empty() {
            return Err(AppError::bad_request("La pregunta no puede estar vacía."));
        }
        tracing::info!(
            characters = cleaned.chars().count(),
            "consulta de soporte recibida"
        );

        let (active_session, started_new) = {
            let conversations = self.conversations.lock().map_err(|_| {
                AppError::internal("lock de conversaciones envenenado", CHAT_INTERNAL)
            })?;
            conversations.resolve_session(session_id.as_deref())
        };
        let history_for_claude: Vec<ChatMessage> = if started_new { Vec::new() } else { history };
        let (hits, sources) = self.retrieve(&cleaned, &history_for_claude).await;
        let context = build_context(&hits, &self.settings.default_document_name);

        let turn = self
            .claude
            .generate_support_reply(
                &cleaned,
                &history_for_claude,
                &active_session,
                &context,
                &self.cases,
            )
            .await?;

        let conversations = self
            .conversations
            .lock()
            .map_err(|_| AppError::internal("lock de conversaciones envenenado", CHAT_INTERNAL))?;
        conversations.record_turn(
            &active_session,
            &cleaned,
            &turn.text,
            &history_for_claude,
            turn.case.as_ref(),
        )?;
        let case_summary = turn.case.as_ref().map(Into::into);
        Ok(ChatResponse {
            answer: turn.text,
            sources,
            session_id: Some(active_session),
            case: case_summary.clone(),
            case_closed: case_summary.is_some(),
            new_chat: started_new,
        })
    }

    async fn retrieve(
        &self,
        question: &str,
        history: &[ChatMessage],
    ) -> (Vec<Hit>, Vec<crate::models::SourceChunk>) {
        let query = build_retrieval_query(question, history);
        let mut vectors = self.vectors.lock().await;
        match vectors.query(&query, self.settings.retrieval_k).await {
            Ok(hits) => {
                let sources = crate::vector::VectorService::hits_to_sources(
                    &hits,
                    &self.settings.default_document_name,
                );
                (hits, sources)
            }
            Err(err) => {
                tracing::warn!(error = %err, "no se pudo recuperar contexto");
                (Vec::new(), Vec::new())
            }
        }
    }

    pub async fn reindex(
        &self,
        mut reset: bool,
        mut force: bool,
        only: Option<&Path>,
    ) -> Result<ReindexResponse, AppError> {
        tracing::info!(reset, force, "indexación solicitada");
        if only.is_some() && reset {
            tracing::warn!("indexar un solo archivo no borra el resto de la colección");
            reset = false;
            force = true;
        }
        let files = if let Some(path) = only {
            vec![path.to_path_buf()]
        } else {
            self.documents.list_knowledge_files()
        };
        if files.is_empty() {
            return Err(AppError::document(
                "No hay documentos para indexar.",
                "No hay archivos .txt o .md en data/documents/. Copia tu guía ahí y vuelve a indexar.",
            ));
        }

        let mut vectors = self.vectors.lock().await;
        if reset {
            vectors.reset_collection().await?;
        }

        let mut written: i64 = 0;
        let mut characters: i64 = 0;
        let mut indexed_names = Vec::new();
        let mut skipped_names = Vec::new();

        for path in files {
            let service = DocumentService::for_file(
                path.clone(),
                self.documents.documents_directory.clone(),
                self.documents.chunk_options,
            );
            let raw = service.load_raw_text()?;
            let fingerprint = fingerprint_text(&raw);
            characters += raw.chars().count() as i64;
            let name = path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("documento.txt")
                .to_string();
            if !reset
                && !force
                && vectors.source_fingerprint(&name).as_deref() == Some(fingerprint.as_str())
                && vectors.count().await > 0
            {
                skipped_names.push(name);
                continue;
            }
            let chunks = service.chunk_text(&raw, &name)?;
            let stored = vectors
                .index_chunks(&chunks, &fingerprint, false, true)
                .await?;
            written += stored as i64;
            indexed_names.push(name);
        }

        let message = if !indexed_names.is_empty() {
            if reset {
                format!(
                    "Base recreada. Se indexaron {} archivo(s) ({} fragmentos).",
                    indexed_names.len(),
                    written
                )
            } else {
                let mut text = format!(
                    "Se indexaron {} archivo(s) con cambios ({} fragmentos).",
                    indexed_names.len(),
                    written
                );
                if !skipped_names.is_empty() {
                    text.push_str(&format!(" Sin cambios: {}.", skipped_names.join(", ")));
                }
                text
            }
        } else {
            "Nada que indexar: los archivos ya estaban al día.".to_string()
        };
        let listed = if indexed_names.is_empty() {
            skipped_names.clone()
        } else {
            indexed_names.clone()
        };
        Ok(ReindexResponse {
            status: "ok".to_string(),
            message,
            chunks_indexed: written,
            characters,
            source: listed.join(", "),
            sources: listed,
            files_indexed: indexed_names.len() as i64,
            files_skipped: skipped_names.len() as i64,
            reset,
        })
    }

    pub async fn index_status(&self) -> Result<IndexStatusResponse, AppError> {
        let files = self.documents.list_knowledge_files();
        let mut vectors = self.vectors.lock().await;
        let mut items = Vec::new();
        let mut pending = false;
        for path in files {
            let service = DocumentService::for_file(
                path.clone(),
                self.documents.documents_directory.clone(),
                self.documents.chunk_options,
            );
            let name = path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("documento.txt")
                .to_string();
            let raw = match service.load_raw_text() {
                Ok(text) => text,
                Err(_) => {
                    pending = true;
                    items.push(IndexFileStatus {
                        name,
                        status: "error".to_string(),
                        chunks: 0,
                        characters: 0,
                        indexed_at: String::new(),
                    });
                    continue;
                }
            };
            let fingerprint = fingerprint_text(&raw);
            let stored = vectors.source_meta(&name);
            let status = if stored.is_empty() {
                pending = true;
                "new"
            } else if json_str(stored.get("sha256")).as_deref() != Some(fingerprint.as_str()) {
                pending = true;
                "changed"
            } else {
                "indexed"
            };
            items.push(IndexFileStatus {
                name,
                status: status.to_string(),
                chunks: stored.get("chunks").and_then(json_i64).unwrap_or(0),
                characters: raw.chars().count() as i64,
                indexed_at: json_str(stored.get("indexed_at")).unwrap_or_default(),
            });
        }
        let indexed_chunks = vectors.count().await as i64;
        Ok(IndexStatusResponse {
            directory: self.documents.documents_directory.display().to_string(),
            indexed_chunks,
            pending_changes: pending,
            files: items,
        })
    }

    pub async fn health(&self) -> crate::models::HealthResponse {
        let mut vectors = self.vectors.lock().await;
        let (vector_database, indexed_chunks, status) = if vectors.is_connected().await {
            ("connected", vectors.count().await as i64, "ok")
        } else {
            ("error", 0, "degraded")
        };
        let claude_api = if self.claude.is_configured() {
            "configured"
        } else {
            "missing_api_key"
        };
        crate::models::HealthResponse {
            status: status.to_string(),
            vector_database: vector_database.to_string(),
            indexed_chunks,
            claude_api: claude_api.to_string(),
        }
    }

    pub fn list_cases(&self, limit: usize) -> Result<Vec<crate::cases::CaseRecord>, AppError> {
        let cases = self.cases.lock().map_err(|_| {
            AppError::internal(
                "lock de casos envenenado",
                "Ocurrió un error interno. Revisa la consola del servidor.",
            )
        })?;
        Ok(cases.list_cases(limit))
    }

    pub fn list_conversations(
        &self,
        limit: usize,
    ) -> Result<Vec<crate::models::ConversationSummary>, AppError> {
        let conversations = self.conversations.lock().map_err(|_| {
            AppError::internal(
                "lock de conversaciones envenenado",
                "Ocurrió un error interno. Revisa la consola del servidor.",
            )
        })?;
        Ok(conversations
            .list_conversations(limit)?
            .iter()
            .map(|record| record.to_summary())
            .collect())
    }

    pub fn conversation(
        &self,
        session_id: &str,
    ) -> Result<crate::models::ConversationDetail, AppError> {
        let conversations = self.conversations.lock().map_err(|_| {
            AppError::internal(
                "lock de conversaciones envenenado",
                "Ocurrió un error interno. Revisa la consola del servidor.",
            )
        })?;
        match conversations.get(session_id)? {
            Some(record) => Ok(record.to_detail()),
            None => Err(AppError::not_found("No encontramos esa conversación.")),
        }
    }
}

pub fn build_context(hits: &[Hit], default_source: &str) -> String {
    if hits.is_empty() {
        return String::new();
    }
    let mut sections = Vec::new();
    for (offset, hit) in hits.iter().enumerate() {
        let index = offset + 1;
        let chunk_id = hit
            .metadata
            .get("chunk_id")
            .and_then(json_i64)
            .unwrap_or(index as i64);
        let title = json_str(hit.metadata.get("title"))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| format!("Fragmento {chunk_id}"));
        let source = hit
            .metadata
            .get("source")
            .and_then(|value| value.as_str())
            .unwrap_or(default_source);
        let score_line = hit
            .distance
            .map(|distance| format!(" | relevancia (distancia): {distance:.4}"))
            .unwrap_or_default();
        let body = hit.document.trim();
        sections.push(format!(
            "[Chunk {index} | id={chunk_id} | fuente={source}{score_line} | {title}]\n{body}"
        ));
    }
    sections.join("\n\n")
}

pub fn build_retrieval_query(question: &str, history: &[ChatMessage]) -> String {
    let mut parts = Vec::new();
    let start = history.len().saturating_sub(6);
    for item in &history[start..] {
        if item.role.trim() != "user" {
            continue;
        }
        let text = item.content.trim();
        if !text.is_empty() {
            parts.push(text.to_string());
        }
    }
    let cleaned = question.trim();
    if !cleaned.is_empty() {
        parts.push(cleaned.to_string());
    }
    let mut deduped = Vec::new();
    for part in parts {
        if deduped.last().map(String::as_str) != Some(part.as_str()) {
            deduped.push(part);
        }
    }
    let start = deduped.len().saturating_sub(3);
    deduped[start..].join("\n")
}

#[cfg(test)]
pub fn runtime_settings_for_tests(root: &std::path::Path, settings: &Settings) -> Settings {
    let mut copy = settings.clone();
    copy.chroma_url.clear();
    copy.anthropic_api_key.clear();
    copy.chroma_persist_directory = root.join("chroma");
    copy.index_meta_path = copy.chroma_persist_directory.join(".index_meta.json");
    copy.documents_directory = root.join("documents");
    copy.default_document_path = copy.documents_directory.join("documento.txt");
    copy.cases_directory = root.join("cases");
    copy.cases_file = copy.cases_directory.join("casos.jsonl");
    copy.conversations_directory = copy.cases_directory.join("conversations");
    copy
}

#[cfg(test)]
mod tests {
    use super::build_retrieval_query;
    use crate::models::ChatMessage;

    #[test]
    fn retrieval_query_includes_recent_user_turns() {
        let query = build_retrieval_query(
            "no pude renovarlo",
            &[
                ChatMessage {
                    role: "assistant".to_string(),
                    content: "¿En qué te ayudo?".to_string(),
                },
                ChatMessage {
                    role: "user".to_string(),
                    content: "Tengo un problema con mi dominio".to_string(),
                },
                ChatMessage {
                    role: "assistant".to_string(),
                    content: "Cuéntame un poco más".to_string(),
                },
            ],
        );
        assert!(query.contains("dominio"));
        assert!(query.contains("no pude renovarlo"));
    }
}
