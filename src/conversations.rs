//! Historial de chats, un JSON por sesión. Porta `app/services/conversation_service.py`.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::cases::{now_stamp, CaseRecord};
use crate::config::Settings;
use crate::error::AppError;
use crate::models::{ChatMessage, ConversationDetail, ConversationMessage, ConversationSummary};

static SAFE_SESSION_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z0-9._-]{6,96}$").expect("session regex"));

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConversationRecord {
    pub session_id: String,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
    #[serde(default = "default_status")]
    pub status: String,
    #[serde(default)]
    pub case_id: String,
    #[serde(default)]
    pub correo: String,
    #[serde(default)]
    pub categoria: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub messages: Vec<ConversationMessage>,
}

fn default_status() -> String {
    "open".to_string()
}

impl ConversationRecord {
    pub fn to_summary(&self) -> ConversationSummary {
        ConversationSummary {
            session_id: self.session_id.clone(),
            created_at: self.created_at.clone(),
            updated_at: self.updated_at.clone(),
            status: self.status.clone(),
            case_id: self.case_id.clone(),
            correo: self.correo.clone(),
            categoria: self.categoria.clone(),
            summary: self.summary.clone(),
            message_count: self.messages.len(),
        }
    }

    pub fn to_detail(&self) -> ConversationDetail {
        ConversationDetail {
            summary: self.to_summary(),
            messages: self.messages.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ConversationService {
    pub store_dir: PathBuf,
}

impl ConversationService {
    pub fn new(settings: &Settings) -> Self {
        Self {
            store_dir: settings.conversations_directory.clone(),
        }
    }

    pub fn with_dir(dir: PathBuf) -> Self {
        Self { store_dir: dir }
    }

    pub fn resolve_session(&self, requested: Option<&str>) -> (String, bool) {
        if let Some(requested) = requested {
            if is_safe_session_id(requested) {
                match self.get(requested) {
                    Ok(Some(existing)) if existing.status == "closed" => {}
                    Ok(_) => return (requested.to_string(), false),
                    Err(_) => return (requested.to_string(), false),
                }
            }
        }
        (new_session_id(), true)
    }

    pub fn get(&self, session_id: &str) -> Result<Option<ConversationRecord>, AppError> {
        let Some(path) = self.path_for(session_id) else {
            return Ok(None);
        };
        if !path.exists() {
            return Ok(None);
        }
        let text = std::fs::read_to_string(&path).map_err(|err| {
            tracing::warn!(%session_id, error = %err, "no se pudo leer la conversación");
            AppError::internal(
                format!("No se pudo leer la conversación {session_id}: {err}"),
                "Ocurrió un error interno al generar la respuesta. Revisa la consola del servidor.",
            )
        })?;
        match serde_json::from_str::<ConversationRecord>(&text) {
            Ok(mut record) => {
                if record.session_id.is_empty() {
                    record.session_id = session_id.to_string();
                }
                Ok(Some(record))
            }
            Err(err) => {
                tracing::warn!(%session_id, error = %err, "json de conversación inválido");
                Ok(None)
            }
        }
    }

    pub fn record_turn(
        &self,
        session_id: &str,
        question: &str,
        answer: &str,
        history: &[ChatMessage],
        case: Option<&CaseRecord>,
    ) -> Result<ConversationRecord, AppError> {
        let now = now_stamp();
        let mut record = match self.get(session_id)? {
            Some(existing) => existing,
            None => ConversationRecord {
                session_id: session_id.to_string(),
                created_at: now.clone(),
                updated_at: now.clone(),
                status: "open".to_string(),
                case_id: String::new(),
                correo: String::new(),
                categoria: String::new(),
                summary: String::new(),
                messages: seed_messages(history, &now),
            },
        };
        record.messages.push(ConversationMessage {
            role: "user".to_string(),
            content: question.to_string(),
            created_at: now.clone(),
        });
        record.messages.push(ConversationMessage {
            role: "assistant".to_string(),
            content: answer.to_string(),
            created_at: now.clone(),
        });
        record.updated_at = now;
        if let Some(case) = case {
            record.status = "closed".to_string();
            record.case_id = case.case_id.clone();
            record.correo = case.correo.clone();
            record.categoria = case.categoria.clone();
        }
        record.summary = build_summary(&record);
        self.write(&record)?;
        tracing::info!(
            session_id = %record.session_id,
            status = %record.status,
            messages = record.messages.len(),
            "conversación actualizada"
        );
        Ok(record)
    }

    pub fn list_conversations(&self, limit: usize) -> Result<Vec<ConversationRecord>, AppError> {
        std::fs::create_dir_all(&self.store_dir).map_err(|err| {
            AppError::internal(
                format!("No se pudo abrir el historial: {err}"),
                "Ocurrió un error interno. Revisa la consola del servidor.",
            )
        })?;
        let mut records = Vec::new();
        let entries = std::fs::read_dir(&self.store_dir).map_err(|err| {
            AppError::internal(
                format!("No se pudo listar el historial: {err}"),
                "Ocurrió un error interno. Revisa la consola del servidor.",
            )
        })?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|value| value.to_str()) else {
                continue;
            };
            if let Some(record) = self.get(stem)? {
                records.push(record);
            }
        }
        records.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
        let limit = limit.clamp(1, 200);
        records.truncate(limit);
        Ok(records)
    }

    fn write(&self, record: &ConversationRecord) -> Result<(), AppError> {
        let Some(path) = self.path_for(&record.session_id) else {
            return Err(AppError::bad_request(
                "session_id inválido para guardar la conversación.",
            ));
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|err| {
                AppError::internal(
                    format!("No se pudo crear el historial: {err}"),
                    "Ocurrió un error interno al generar la respuesta. Revisa la consola del servidor.",
                )
            })?;
        }
        let payload = serde_json::to_string_pretty(record).map_err(|err| {
            AppError::internal(
                format!("No se pudo serializar la conversación: {err}"),
                "Ocurrió un error interno al generar la respuesta. Revisa la consola del servidor.",
            )
        })?;
        std::fs::write(path, payload).map_err(|err| {
            AppError::internal(
                format!("No se pudo guardar la conversación: {err}"),
                "Ocurrió un error interno al generar la respuesta. Revisa la consola del servidor.",
            )
        })
    }

    fn path_for(&self, session_id: &str) -> Option<PathBuf> {
        if !is_safe_session_id(session_id) {
            return None;
        }
        Some(self.store_dir.join(format!("{session_id}.json")))
    }
}

pub fn new_session_id() -> String {
    let stamp = crate::cases::colombia_now().format("%Y%m%d%H%M%S");
    let suffix = &Uuid::new_v4().simple().to_string()[..8];
    format!("ses-{stamp}-{suffix}")
}

pub fn is_safe_session_id(value: &str) -> bool {
    let raw = value.trim();
    if !SAFE_SESSION_ID.is_match(raw) {
        return false;
    }
    Path::new(raw).file_name().and_then(|name| name.to_str()) == Some(raw)
}

fn seed_messages(history: &[ChatMessage], stamp: &str) -> Vec<ConversationMessage> {
    let mut seeded = Vec::new();
    for item in history {
        let role = item.role.trim();
        let content = item.content.trim();
        if (role != "user" && role != "assistant") || content.is_empty() {
            continue;
        }
        seeded.push(ConversationMessage {
            role: role.to_string(),
            content: content.to_string(),
            created_at: stamp.to_string(),
        });
    }
    seeded
}

fn build_summary(record: &ConversationRecord) -> String {
    if !record.categoria.is_empty() && !record.correo.is_empty() {
        return format!("{} · {}", record.categoria, record.correo);
    }
    if !record.categoria.is_empty() {
        return record.categoria.clone();
    }
    for message in &record.messages {
        if message.role == "user" {
            let text = message
                .content
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            if text.is_empty() {
                continue;
            }
            let count = text.chars().count();
            if count > 140 {
                let truncated: String = text.chars().take(140).collect();
                return format!("{truncated}…");
            }
            return text;
        }
    }
    "Conversación de soporte".to_string()
}

#[cfg(test)]
mod tests {
    use super::ConversationService;
    use crate::cases::CaseRecord;
    use crate::models::ChatMessage;
    use std::path::PathBuf;

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("micomco-conv-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn record_turn_persists_full_transcript() {
        let dir = temp_dir();
        let store = ConversationService::with_dir(dir.clone());
        store
            .record_turn(
                "ses-demo-abc123",
                "Hola, mi correo es ana@empresa.co",
                "Gracias, Ana. ¿El tema es correo, dominio, hosting o facturación?",
                &[ChatMessage {
                    role: "assistant".to_string(),
                    content: "¿Me compartes el correo con el que estás registrado?".to_string(),
                }],
                None,
            )
            .unwrap();
        let stored = store.get("ses-demo-abc123").unwrap().unwrap();
        assert_eq!(stored.status, "open");
        assert_eq!(stored.messages.len(), 3);
        assert!(stored.messages[1].content.starts_with("Hola"));
        let listed = store.list_conversations(50).unwrap();
        assert_eq!(listed[0].session_id, "ses-demo-abc123");
        assert!(listed[0].summary.contains("ana@empresa.co"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn closed_session_opens_a_new_chat() {
        let dir = temp_dir();
        let store = ConversationService::with_dir(dir.clone());
        let case = CaseRecord {
            case_id: "CASO-1".to_string(),
            correo: "ana@empresa.co".to_string(),
            categoria: "Hosting".to_string(),
            detalle: "No entra a cPanel".to_string(),
            registrado_en: "2026-08-25 09:00:00 COT".to_string(),
            estado: "Cerrado/Registrado".to_string(),
            session_id: "ses-old-closed1".to_string(),
        };
        store
            .record_turn(
                "ses-old-closed1",
                "Sí, confirma el caso",
                "Quedó registrado. Hasta pronto.",
                &[],
                Some(&case),
            )
            .unwrap();
        let closed = store.get("ses-old-closed1").unwrap().unwrap();
        assert_eq!(closed.status, "closed");
        let (session_id, started_new) = store.resolve_session(Some("ses-old-closed1"));
        assert!(started_new);
        assert_ne!(session_id, "ses-old-closed1");
        let _ = std::fs::remove_dir_all(dir);
    }
}
