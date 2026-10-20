//! Historial persistente de cada chat (un JSON por sesión).

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};

use regex::Regex;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::cases::{format_colombia, now_colombia, CaseRecord};

static SAFE_SESSION_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z0-9._-]{6,96}$").expect("session id"));

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredMessage {
    pub role: String,
    pub content: String,
    #[serde(default)]
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationRecord {
    pub session_id: String,
    pub created_at: String,
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
    pub messages: Vec<StoredMessage>,
}

fn default_status() -> String {
    "open".to_string()
}

#[derive(Debug, Clone, Serialize)]
pub struct ConversationListItem {
    pub session_id: String,
    pub created_at: String,
    pub updated_at: String,
    pub status: String,
    pub case_id: String,
    pub correo: String,
    pub categoria: String,
    pub summary: String,
    pub message_count: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConversationDetail {
    pub session_id: String,
    pub created_at: String,
    pub updated_at: String,
    pub status: String,
    pub case_id: String,
    pub correo: String,
    pub categoria: String,
    pub summary: String,
    pub message_count: usize,
    pub messages: Vec<StoredMessage>,
}

impl ConversationRecord {
    pub fn to_list_item(&self) -> ConversationListItem {
        ConversationListItem {
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
        let item = self.to_list_item();
        ConversationDetail {
            session_id: item.session_id,
            created_at: item.created_at,
            updated_at: item.updated_at,
            status: item.status,
            case_id: item.case_id,
            correo: item.correo,
            categoria: item.categoria,
            summary: item.summary,
            message_count: item.message_count,
            messages: self.messages.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct HistoryTurn {
    pub role: String,
    pub content: String,
}

#[derive(Clone)]
pub struct ConversationService {
    store_dir: PathBuf,
    lock: Arc<Mutex<()>>,
}

impl ConversationService {
    pub fn new(store_dir: impl Into<PathBuf>) -> Self {
        Self {
            store_dir: store_dir.into(),
            lock: Arc::new(Mutex::new(())),
        }
    }

    /// Devuelve un session_id activo. Si el chat ya se cerró, abre uno nuevo.
    pub fn resolve_session(&self, requested: Option<&str>) -> (String, bool) {
        if let Some(raw) = requested.and_then(normalize_session_id) {
            match self.get(&raw) {
                None => return (raw, false),
                Some(existing) if existing.status != "closed" => return (raw, false),
                Some(_) => {}
            }
        }
        (new_session_id(), true)
    }

    pub fn get(&self, session_id: &str) -> Option<ConversationRecord> {
        let _guard = self.lock.lock().unwrap_or_else(|err| err.into_inner());
        let path = path_for(&self.store_dir, session_id)?;
        if !path.exists() {
            return None;
        }
        let text = fs::read_to_string(&path).ok()?;
        let payload: serde_json::Value = serde_json::from_str(&text).ok()?;
        Some(record_from_value(&payload, session_id))
    }

    pub fn record_turn(
        &self,
        session_id: &str,
        question: &str,
        answer: &str,
        history: &[HistoryTurn],
        case: Option<&CaseRecord>,
    ) -> Result<ConversationRecord, String> {
        let _guard = self.lock.lock().unwrap_or_else(|err| err.into_inner());
        if path_for(&self.store_dir, session_id).is_none() {
            return Err("session_id inválido para guardar la conversación.".into());
        }
        let now = format_colombia(now_colombia());
        let mut record = self
            .get_unlocked(session_id)
            .unwrap_or_else(|| ConversationRecord {
                session_id: session_id.to_string(),
                created_at: now.clone(),
                updated_at: now.clone(),
                status: "open".into(),
                case_id: String::new(),
                correo: String::new(),
                categoria: String::new(),
                summary: String::new(),
                messages: seed_messages(history, &now),
            });
        record.messages.push(StoredMessage {
            role: "user".into(),
            content: question.to_string(),
            created_at: now.clone(),
        });
        record.messages.push(StoredMessage {
            role: "assistant".into(),
            content: answer.to_string(),
            created_at: now.clone(),
        });
        record.updated_at = now;
        if let Some(case) = case {
            record.status = "closed".into();
            record.case_id = case.case_id.clone();
            record.correo = case.correo.clone();
            record.categoria = case.categoria.clone();
        }
        record.summary = build_summary(&record);
        self.write_unlocked(&record)?;
        tracing::info!(
            "Conversación {} actualizada ({}, {} mensajes).",
            record.session_id,
            record.status,
            record.messages.len()
        );
        Ok(record)
    }

    pub fn list_conversations(&self, limit: usize) -> Vec<ConversationRecord> {
        let _guard = self.lock.lock().unwrap_or_else(|err| err.into_inner());
        let _ = fs::create_dir_all(&self.store_dir);
        let mut records = Vec::new();
        let Ok(entries) = fs::read_dir(&self.store_dir) else {
            return records;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
                continue;
            };
            if let Some(record) = self.get_unlocked(stem) {
                records.push(record);
            }
        }
        records.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
        let limit = limit.clamp(1, 200);
        records.truncate(limit);
        records
    }

    fn get_unlocked(&self, session_id: &str) -> Option<ConversationRecord> {
        let path = path_for(&self.store_dir, session_id)?;
        if !path.exists() {
            return None;
        }
        let text = fs::read_to_string(path).ok()?;
        let payload: serde_json::Value = serde_json::from_str(&text).ok()?;
        Some(record_from_value(&payload, session_id))
    }

    fn write_unlocked(&self, record: &ConversationRecord) -> Result<(), String> {
        let path = path_for(&self.store_dir, &record.session_id)
            .ok_or_else(|| "session_id inválido para guardar la conversación.".to_string())?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|err| err.to_string())?;
        }
        let body = serde_json::to_string_pretty(record).map_err(|err| err.to_string())?;
        fs::write(path, body).map_err(|err| err.to_string())
    }
}

pub fn new_session_id() -> String {
    let stamp = now_colombia().format("%Y%m%d%H%M%S");
    let suffix = &Uuid::new_v4().simple().to_string()[..8];
    format!("ses-{stamp}-{suffix}")
}

pub fn normalize_session_id(value: &str) -> Option<String> {
    let raw = value.trim();
    if SAFE_SESSION_ID.is_match(raw)
        && Path::new(raw).file_name().and_then(|name| name.to_str()) == Some(raw)
    {
        Some(raw.to_string())
    } else {
        None
    }
}

fn path_for(store_dir: &Path, session_id: &str) -> Option<PathBuf> {
    let raw = normalize_session_id(session_id)?;
    Some(store_dir.join(format!("{raw}.json")))
}

fn seed_messages(history: &[HistoryTurn], stamp: &str) -> Vec<StoredMessage> {
    history
        .iter()
        .filter_map(|item| {
            let role = item.role.trim();
            let content = item.content.trim();
            if (role != "user" && role != "assistant") || content.is_empty() {
                return None;
            }
            Some(StoredMessage {
                role: role.to_string(),
                content: content.to_string(),
                created_at: stamp.to_string(),
            })
        })
        .collect()
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
            if !text.is_empty() {
                return truncate_summary(&text);
            }
        }
    }
    "Conversación de soporte".to_string()
}

fn truncate_summary(text: &str) -> String {
    let count = text.chars().count();
    if count > 140 {
        let cut: String = text.chars().take(140).collect();
        format!("{cut}…")
    } else {
        text.to_string()
    }
}

fn record_from_value(payload: &serde_json::Value, fallback_id: &str) -> ConversationRecord {
    let messages = payload
        .get("messages")
        .and_then(|value| serde_json::from_value::<Vec<StoredMessage>>(value.clone()).ok())
        .unwrap_or_default();
    ConversationRecord {
        session_id: json_str(payload, "session_id").unwrap_or_else(|| fallback_id.to_string()),
        created_at: json_str(payload, "created_at").unwrap_or_default(),
        updated_at: json_str(payload, "updated_at").unwrap_or_default(),
        status: json_str(payload, "status").unwrap_or_else(|| "open".into()),
        case_id: json_str(payload, "case_id").unwrap_or_default(),
        correo: json_str(payload, "correo").unwrap_or_default(),
        categoria: json_str(payload, "categoria").unwrap_or_default(),
        summary: json_str(payload, "summary").unwrap_or_default(),
        messages,
    }
}

fn json_str(payload: &serde_json::Value, key: &str) -> Option<String> {
    payload.get(key).and_then(|value| match value {
        serde_json::Value::String(text) => Some(text.clone()),
        serde_json::Value::Null => None,
        other => Some(other.to_string()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_turn_persists_full_transcript() {
        let dir = tempfile::tempdir().unwrap();
        let store = ConversationService::new(dir.path().join("conversations"));
        store
            .record_turn(
                "ses-demo-abc123",
                "Hola, mi correo es ana@empresa.co",
                "Gracias, Ana. ¿El tema es correo, dominio, hosting o facturación?",
                &[HistoryTurn {
                    role: "assistant".into(),
                    content: "¿Me compartes el correo con el que estás registrado?".into(),
                }],
                None,
            )
            .unwrap();
        let stored = store.get("ses-demo-abc123").unwrap();
        assert_eq!(stored.status, "open");
        assert_eq!(stored.messages.len(), 3);
        assert!(stored.messages[1].content.starts_with("Hola"));
        let listed = store.list_conversations(50);
        assert_eq!(listed[0].session_id, "ses-demo-abc123");
        assert!(listed[0].summary.contains("ana@empresa.co"));
    }

    #[test]
    fn closed_session_opens_a_new_chat() {
        let dir = tempfile::tempdir().unwrap();
        let store = ConversationService::new(dir.path().join("conversations"));
        let case = CaseRecord {
            case_id: "CASO-1".into(),
            correo: "ana@empresa.co".into(),
            categoria: "Hosting".into(),
            detalle: "No entra a cPanel".into(),
            registrado_en: "2026-08-25 09:00:00 COT".into(),
            estado: "Cerrado/Registrado".into(),
            session_id: "ses-old-closed1".into(),
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
        let closed = store.get("ses-old-closed1").unwrap();
        assert_eq!(closed.status, "closed");
        let (session_id, started_new) = store.resolve_session(Some("ses-old-closed1"));
        assert!(started_new);
        assert_ne!(session_id, "ses-old-closed1");
    }
}
