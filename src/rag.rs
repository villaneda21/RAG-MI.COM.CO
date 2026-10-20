//! Orquestación del flujo RAG: validar, recuperar, contextualizar y generar.

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Value;

use crate::cases::CaseService;
use crate::claude::{ClaudeServiceError, SupportClient};
use crate::config::Settings;
use crate::conversations::{ConversationService, HistoryTurn};
use crate::documents::DocumentService;
use crate::models::{
    ChatResponse, Hit, IndexFileStatus, IndexStatusResponse, ReindexResponse, SourceChunk,
};
use crate::text::DocumentProcessingError;
use crate::vector::{meta_i64, meta_string, VectorService, VectorStoreError};

#[derive(Debug, thiserror::Error)]
pub enum RagError {
    #[error("{0}")]
    BadRequest(String),
    #[error(transparent)]
    Document(#[from] DocumentProcessingError),
    #[error(transparent)]
    Vector(#[from] VectorStoreError),
    #[error(transparent)]
    Claude(#[from] ClaudeServiceError),
    #[error("{0}")]
    Internal(String),
}

#[derive(Clone)]
pub struct RagService {
    pub settings: Settings,
    pub documents: DocumentService,
    pub vector: VectorService,
    pub claude: Arc<dyn SupportClient>,
    pub cases: CaseService,
    pub conversations: ConversationService,
}

impl RagService {
    pub fn build_context(&self, hits: &[Hit]) -> String {
        if hits.is_empty() {
            return String::new();
        }
        let mut sections = Vec::new();
        for (offset, hit) in hits.iter().enumerate() {
            let index = offset + 1;
            let chunk_id = meta_i64(&hit.metadata, "chunk_id").unwrap_or(index as i64);
            let title = meta_string(&hit.metadata, "title")
                .unwrap_or_else(|| format!("Fragmento {chunk_id}"));
            let source = meta_string(&hit.metadata, "source")
                .unwrap_or_else(|| self.settings.default_document_name.clone());
            let score_line = match hit.distance {
                Some(distance) => format!(" | relevancia (distancia): {distance:.4}"),
                None => String::new(),
            };
            let body = hit.document.trim();
            sections.push(format!(
                "[Chunk {index} | id={chunk_id} | fuente={source}{score_line} | {title}]\n{body}"
            ));
        }
        sections.join("\n\n")
    }

    pub async fn ask(
        &self,
        question: &str,
        history: &[HistoryTurn],
        session_id: Option<&str>,
    ) -> Result<ChatResponse, RagError> {
        self.ask_with_session(question, history, session_id, "")
            .await
    }

    pub async fn ask_with_session(
        &self,
        question: &str,
        history: &[HistoryTurn],
        session_id: Option<&str>,
        session_note: &str,
    ) -> Result<ChatResponse, RagError> {
        let cleaned = question.trim();
        if cleaned.is_empty() {
            return Err(RagError::BadRequest(
                "La pregunta no puede estar vacía.".into(),
            ));
        }
        tracing::info!(
            "Consulta de soporte recibida ({} caracteres).",
            cleaned.chars().count()
        );
        let (active_session, started_new) = self.conversations.resolve_session(session_id);
        let history_for_claude = if started_new {
            Vec::new()
        } else {
            history.to_vec()
        };
        let (hits, sources) = self.retrieve(cleaned, &history_for_claude).await;
        let context = self.build_context(&hits);
        let turn = self
            .claude
            .generate_support_reply(
                cleaned.to_string(),
                history_for_claude.clone(),
                active_session.clone(),
                self.cases.clone(),
                context,
                session_note.to_string(),
            )
            .await?;
        let case_summary = turn.case.as_ref().map(|case| case.summary());
        self.conversations
            .record_turn(
                &active_session,
                cleaned,
                &turn.text,
                &history_for_claude,
                turn.case.as_ref(),
            )
            .map_err(|err| {
                tracing::error!("No se pudo guardar la conversación: {err}");
                RagError::Internal(
                    "Ocurrió un error interno al generar la respuesta. Revisa la consola del servidor."
                        .into(),
                )
            })?;
        Ok(ChatResponse {
            answer: turn.text,
            sources,
            session_id: Some(active_session),
            case_closed: case_summary.is_some(),
            case: case_summary,
            new_chat: started_new,
        })
    }

    async fn retrieve(
        &self,
        question: &str,
        history: &[HistoryTurn],
    ) -> (Vec<Hit>, Vec<SourceChunk>) {
        let query = build_retrieval_query(question, history);
        match self.vector.query(&query, self.settings.retrieval_k).await {
            Ok(hits) => {
                let sources = self.vector.hits_to_sources(&hits);
                (hits, sources)
            }
            Err(err) => {
                tracing::warn!("No se pudo recuperar contexto de la base de conocimiento: {err}");
                (Vec::new(), Vec::new())
            }
        }
    }

    pub async fn reindex(
        &self,
        mut reset: bool,
        mut force: bool,
        only: Option<PathBuf>,
    ) -> Result<ReindexResponse, RagError> {
        tracing::info!("Indexación solicitada (reset={reset}, force={force}, only={only:?}).");
        if only.is_some() && reset {
            tracing::warn!("Indexar un solo archivo no borra el resto de la colección.");
            reset = false;
            force = true;
        }
        let files = if let Some(path) = only {
            vec![path]
        } else {
            self.documents.list_knowledge_files()
        };
        if files.is_empty() {
            return Err(DocumentProcessingError::new(
                "No hay documentos para indexar.",
                "No hay archivos .txt o .md en data/documents/. Copia tu guía ahí y vuelve a indexar.",
            )
            .into());
        }
        if reset {
            self.vector.reset_collection().await?;
        }

        let mut written = 0usize;
        let mut characters = 0usize;
        let mut indexed_names = Vec::new();
        let mut skipped_names = Vec::new();

        for path in files {
            let service = self.documents.for_file(&path);
            let raw = service.load_raw_text()?;
            let fingerprint = service.fingerprint(&raw);
            characters += raw.chars().count();
            let name = path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("documento.txt")
                .to_string();
            if !reset
                && !force
                && self.vector.source_fingerprint(&name).as_deref() == Some(fingerprint.as_str())
                && self.vector.count().await > 0
            {
                skipped_names.push(name);
                continue;
            }
            let chunks = service.chunk_text(&raw, Some(&name))?;
            let stored = self
                .vector
                .index_chunks(&chunks, &fingerprint, false, true)
                .await?;
            written += stored;
            indexed_names.push(name);
        }

        let message = if !indexed_names.is_empty() {
            if reset {
                format!(
                    "Base recreada. Se indexaron {} archivo(s) ({written} fragmentos).",
                    indexed_names.len()
                )
            } else {
                let mut text = format!(
                    "Se indexaron {} archivo(s) con cambios ({written} fragmentos).",
                    indexed_names.len()
                );
                if !skipped_names.is_empty() {
                    text.push_str(&format!(" Sin cambios: {}.", skipped_names.join(", ")));
                }
                text
            }
        } else {
            "Nada que indexar: los archivos ya estaban al día.".to_string()
        };
        let names = if indexed_names.is_empty() {
            skipped_names.clone()
        } else {
            indexed_names.clone()
        };
        Ok(ReindexResponse {
            status: "ok".into(),
            message,
            chunks_indexed: written,
            characters,
            source: names.join(", "),
            files_indexed: indexed_names.len(),
            files_skipped: skipped_names.len(),
            sources: names,
            reset,
        })
    }

    pub async fn index_status(&self) -> IndexStatusResponse {
        let files = self.documents.list_knowledge_files();
        let mut items = Vec::new();
        let mut pending = false;
        for path in files {
            let service = self.documents.for_file(&path);
            let name = path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("documento.txt")
                .to_string();
            let raw = match service.load_raw_text() {
                Ok(raw) => raw,
                Err(_) => {
                    pending = true;
                    items.push(IndexFileStatus {
                        name,
                        status: "error".into(),
                        chunks: 0,
                        characters: 0,
                        indexed_at: String::new(),
                    });
                    continue;
                }
            };
            let fingerprint = service.fingerprint(&raw);
            let stored = self.vector.source_meta(&name);
            let status = if stored.is_empty() {
                pending = true;
                "new"
            } else if stored.get("sha256").and_then(Value::as_str) != Some(fingerprint.as_str()) {
                pending = true;
                "changed"
            } else {
                "indexed"
            };
            items.push(IndexFileStatus {
                name,
                status: status.into(),
                chunks: stored
                    .get("chunks")
                    .and_then(|value| value.as_i64())
                    .unwrap_or(0),
                characters: raw.chars().count(),
                indexed_at: stored
                    .get("indexed_at")
                    .and_then(|value| value.as_str())
                    .unwrap_or("")
                    .to_string(),
            });
        }
        IndexStatusResponse {
            directory: self.documents.documents_directory.display().to_string(),
            indexed_chunks: self.vector.count().await,
            pending_changes: pending,
            files: items,
        }
    }
}

pub async fn prepare_index(rag: &RagService) -> Result<(), RagError> {
    rag.vector.connect().await?;
    let status = rag.index_status().await;
    let count = rag.vector.count().await;
    if count == 0 || status.pending_changes {
        tracing::info!("Indexando archivos nuevos o modificados en data/documents/ ...");
        let result = rag.reindex(count == 0, false, None).await?;
        tracing::info!("{}", result.message);
    } else {
        tracing::info!("La base vectorial ya tiene {count} fragmentos al día.");
    }
    tracing::info!("Precargando el modelo de embeddings...");
    rag.vector.preload_embeddings().await?;
    tracing::info!("Modelo de embeddings listo.");
    Ok(())
}

pub fn build_retrieval_query(question: &str, history: &[HistoryTurn]) -> String {
    let mut parts = Vec::new();
    for item in history
        .iter()
        .rev()
        .take(6)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
    {
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
        if deduped.last() != Some(&part) {
            deduped.push(part);
        }
    }
    let start = deduped.len().saturating_sub(3);
    deduped[start..].join("\n")
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use async_trait::async_trait;

    use super::*;
    use crate::assemble_rag;
    use crate::claude::{ClaudeServiceError, SupportTurn};
    use crate::embed::HashEmbedder;
    use crate::models::ChatResponse;

    struct ScriptedClaude {
        text: String,
        context: Mutex<String>,
    }

    #[async_trait]
    impl SupportClient for ScriptedClaude {
        fn is_configured(&self) -> bool {
            true
        }

        async fn generate_support_reply(
            &self,
            _question: String,
            _history: Vec<HistoryTurn>,
            _session_id: String,
            _case_service: CaseService,
            context: String,
            session_note: String,
        ) -> Result<SupportTurn, ClaudeServiceError> {
            *self.context.lock().unwrap_or_else(|err| err.into_inner()) =
                format!("{session_note}\n{context}");
            Ok(SupportTurn {
                text: self.text.clone(),
                case: None,
            })
        }
    }

    fn write_sample(dir: &std::path::Path) {
        std::fs::write(
            dir.join("documento.txt"),
            "La política de la empresa indica que el soporte técnico responde en horario continuo.\n\n\
El código de cliente tiene el formato CLI-XXXXXX y se consulta en Mi cuenta.\n\n\
Para acceder a cPanel se debe entrar a Mis servicios y pulsar Acceder a cPanel.\n",
        )
        .unwrap();
    }

    fn rag_with_script(dir: &std::path::Path, text: &str) -> (RagService, Arc<ScriptedClaude>) {
        write_sample(dir);
        let settings = Settings::for_tests(dir);
        let scripted = Arc::new(ScriptedClaude {
            text: text.to_string(),
            context: Mutex::new(String::new()),
        });
        let rag = assemble_rag(
            settings,
            Arc::new(HashEmbedder { dimensions: 32 }),
            Some(scripted.clone() as Arc<dyn SupportClient>),
        );
        (rag, scripted)
    }

    #[test]
    fn retrieval_query_includes_recent_user_turns() {
        let query = build_retrieval_query(
            "no pude renovarlo",
            &[
                HistoryTurn {
                    role: "assistant".into(),
                    content: "¿En qué te ayudo?".into(),
                },
                HistoryTurn {
                    role: "user".into(),
                    content: "Tengo un problema con mi dominio".into(),
                },
                HistoryTurn {
                    role: "assistant".into(),
                    content: "Cuéntame un poco más".into(),
                },
            ],
        );
        assert!(query.contains("dominio"));
        assert!(query.contains("no pude renovarlo"));
    }

    #[tokio::test]
    async fn reindex_and_context() {
        let dir = tempfile::tempdir().unwrap();
        let (rag, _) = rag_with_script(dir.path(), "respuesta");
        rag.vector.connect().await.unwrap();
        let result = rag.reindex(true, false, None).await.unwrap();
        assert_eq!(result.status, "ok");
        assert!(result.chunks_indexed >= 1);
        assert_eq!(rag.vector.count().await, result.chunks_indexed);
        let hits = rag.vector.query("soporte técnico", 5).await.unwrap();
        let context = rag.build_context(&hits);
        assert!(context.contains("[Chunk 1"));
        assert!(context.contains("fuente="));
        assert!(context.len() > 40);
    }

    #[tokio::test]
    async fn ask_uses_scripted_claude_and_passes_guides() {
        let dir = tempfile::tempdir().unwrap();
        let (rag, scripted) = rag_with_script(
            dir.path(),
            "El código de cliente tiene el formato CLI-XXXXXX y se consulta en Mi cuenta.",
        );
        rag.vector.connect().await.unwrap();
        rag.reindex(true, false, None).await.unwrap();
        let response = rag
            .ask("¿Dónde veo el código de cliente?", &[], None)
            .await
            .unwrap();
        assert!(
            response.answer.to_lowercase().contains("código") || response.answer.contains("CLI")
        );
        assert!(response.session_id.is_some());
        let stored = rag
            .conversations
            .get(response.session_id.as_deref().unwrap())
            .unwrap();
        assert_eq!(stored.messages.last().unwrap().content, response.answer);
        let context = scripted.context.lock().unwrap().clone();
        assert!(context.contains("CLI-XXXXXX"));
        assert!(!response.sources.is_empty());
        let _: ChatResponse = response;
    }

    #[tokio::test]
    async fn ask_starts_new_chat_when_previous_case_is_closed() {
        let dir = tempfile::tempdir().unwrap();
        let (rag, _) = rag_with_script(
            dir.path(),
            "Con gusto. Para ubicar tu cuenta, ¿me compartes el correo con el que estás registrado?",
        );
        rag.vector.connect().await.unwrap();
        rag.reindex(true, false, None).await.unwrap();
        let case = crate::cases::CaseRecord {
            case_id: "CASO-99".into(),
            correo: "luis@mi.com.co".into(),
            categoria: "Dominio".into(),
            detalle: "El dominio no abre".into(),
            registrado_en: "2026-08-25 10:00:00 COT".into(),
            estado: "Cerrado/Registrado".into(),
            session_id: "ses-closed-xyz789".into(),
        };
        rag.conversations
            .record_turn(
                "ses-closed-xyz789",
                "Confirma por favor",
                "Caso cerrado. Gracias.",
                &[],
                Some(&case),
            )
            .unwrap();
        let response = rag
            .ask(
                "Hola de nuevo",
                &[HistoryTurn {
                    role: "user".into(),
                    content: "mensaje viejo que no debe reutilizarse".into(),
                }],
                Some("ses-closed-xyz789"),
            )
            .await
            .unwrap();
        assert!(response.new_chat);
        assert_ne!(response.session_id.as_deref(), Some("ses-closed-xyz789"));
    }

    #[tokio::test]
    async fn reindex_adds_a_new_file_without_wiping_the_first() {
        let dir = tempfile::tempdir().unwrap();
        let (rag, _) = rag_with_script(dir.path(), "ok");
        rag.vector.connect().await.unwrap();
        let first = rag.reindex(true, false, None).await.unwrap();
        assert_eq!(first.files_indexed, 1);
        let first_count = rag.vector.count().await;
        std::fs::write(
            dir.path().join("renovacion.txt"),
            "Para renovar un dominio entra a Mis servicios y revisa la fecha de vencimiento.",
        )
        .unwrap();
        let second = rag.reindex(false, false, None).await.unwrap();
        assert_eq!(second.files_indexed, 1);
        assert!(second.sources.iter().any(|name| name == "renovacion.txt"));
        assert!(rag.vector.count().await > first_count);
        let third = rag.reindex(false, false, None).await.unwrap();
        assert_eq!(third.files_indexed, 0);
        assert_eq!(third.files_skipped, 2);
    }

    #[cfg(feature = "semantic-embeddings")]
    #[tokio::test]
    #[ignore = "carga all-MiniLM-L6-v2; comprueba que la búsqueda semántica ordena la guía correcta"]
    async fn minilm_ranks_the_matching_guide() {
        let dir = tempfile::tempdir().unwrap();
        let mut settings = Settings::for_tests(dir.path());
        settings.embedding_model = "all-MiniLM-L6-v2".into();
        settings.fastembed_cache =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".cache/fastembed");
        std::fs::write(
            dir.path().join("documento.txt"),
            "El código de cliente tiene el formato CLI-XXXXXX y se consulta en Mi cuenta.\n\n\
La factura electrónica llega al correo registrado en el portal.\n",
        )
        .unwrap();
        let embedder = Arc::new(crate::embed::FastEmbedder::new(&settings).unwrap());
        let rag = crate::assemble_rag(settings, embedder, None);
        rag.vector.connect().await.unwrap();
        rag.reindex(true, false, None).await.unwrap();
        let hits = rag
            .vector
            .query("¿Dónde veo el código de cliente?", 1)
            .await
            .unwrap();
        assert!(
            hits[0].document.contains("CLI-XXXXXX"),
            "el fragmento más cercano fue: {}",
            hits[0].document
        );
    }

    #[tokio::test]
    async fn index_status_detects_new_and_changed_files() {
        let dir = tempfile::tempdir().unwrap();
        let (rag, _) = rag_with_script(dir.path(), "ok");
        rag.vector.connect().await.unwrap();
        rag.reindex(true, false, None).await.unwrap();
        std::fs::write(
            dir.path().join("extra.txt"),
            "Nueva guía de facturación electrónica.",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("documento.txt"),
            "El código de cliente tiene el formato CLI-XXXXXX y se consulta en Mi cuenta. "
                .repeat(8),
        )
        .unwrap();
        let status = rag.index_status().await;
        let by_name = |name: &str| {
            status
                .files
                .iter()
                .find(|item| item.name == name)
                .map(|item| item.status.as_str())
        };
        assert_eq!(by_name("extra.txt"), Some("new"));
        assert_eq!(by_name("documento.txt"), Some("changed"));
        assert!(status.pending_changes);
    }
}
