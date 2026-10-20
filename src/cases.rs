//! Registro persistente de casos de soporte (JSONL), igual que `case_service.py`.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use chrono_tz::America::Bogota;
use chrono_tz::Tz;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;
use uuid::Uuid;

use crate::models::CaseSummary;

pub const CATEGORIES: &[&str] = &[
    "Correo electrónico",
    "Dominio",
    "Hosting",
    "Facturación o Compras",
];

static EMAIL_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[^@\s]+@[^@\s]+\.[^@\s]+$").expect("email"));

#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct CaseRegistrationError {
    pub message: String,
    pub user_message: String,
}

impl CaseRegistrationError {
    pub fn new(message: impl Into<String>, user_message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            user_message: user_message.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaseRecord {
    pub case_id: String,
    pub correo: String,
    pub categoria: String,
    pub detalle: String,
    pub registrado_en: String,
    pub estado: String,
    #[serde(default)]
    pub session_id: String,
}

impl CaseRecord {
    pub fn summary(&self) -> CaseSummary {
        CaseSummary {
            case_id: self.case_id.clone(),
            correo: self.correo.clone(),
            categoria: self.categoria.clone(),
            detalle: self.detalle.clone(),
            registrado_en: self.registrado_en.clone(),
            estado: self.estado.clone(),
        }
    }
}

#[derive(Clone)]
pub struct CaseService {
    store_path: PathBuf,
    lock: Arc<Mutex<()>>,
}

impl CaseService {
    pub fn new(store_path: impl Into<PathBuf>) -> Self {
        Self {
            store_path: store_path.into(),
            lock: Arc::new(Mutex::new(())),
        }
    }

    pub fn register(
        &self,
        correo: &str,
        categoria: &str,
        detalle: &str,
        session_id: &str,
    ) -> Result<CaseRecord, CaseRegistrationError> {
        let email = correo.trim().to_lowercase();
        let notes = detalle.trim();
        let category = normalize_category(categoria);
        if !EMAIL_PATTERN.is_match(&email) {
            return Err(CaseRegistrationError::new(
                format!("Correo inválido: {correo}"),
                "El correo no parece válido. ¿Me lo confirmas de nuevo?",
            ));
        }
        if !CATEGORIES.contains(&category.as_str()) {
            return Err(CaseRegistrationError::new(
                format!("Categoría inválida: {categoria}"),
                "¿El tema es correo, dominio, hosting o facturación/compras?",
            ));
        }
        if notes.chars().count() < 3 {
            return Err(CaseRegistrationError::new(
                "El detalle del caso está vacío.",
                "Cuéntame un poco más del motivo de tu contacto para dejarlo registrado.",
            ));
        }
        let now = now_colombia();
        let record = CaseRecord {
            case_id: new_case_id(now),
            correo: email,
            categoria: category,
            detalle: notes.to_string(),
            registrado_en: format_colombia(now),
            estado: "Cerrado/Registrado".to_string(),
            session_id: session_id.to_string(),
        };
        self.append(&record)?;
        tracing::info!(
            "Caso {} registrado ({}, {}).",
            record.case_id,
            record.categoria,
            record.correo
        );
        Ok(record)
    }

    pub fn list_cases(&self, limit: usize) -> Vec<CaseRecord> {
        let _guard = self.lock.lock().unwrap_or_else(|err| err.into_inner());
        if !self.store_path.exists() {
            return Vec::new();
        }
        let Ok(text) = fs::read_to_string(&self.store_path) else {
            return Vec::new();
        };
        let mut records = Vec::new();
        for line in text.lines() {
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(record) = serde_json::from_str::<CaseRecord>(line) {
                records.push(record);
            }
        }
        let limit = limit.max(1);
        let start = records.len().saturating_sub(limit);
        records[start..].iter().rev().cloned().collect()
    }

    fn append(&self, record: &CaseRecord) -> Result<(), CaseRegistrationError> {
        let _guard = self.lock.lock().unwrap_or_else(|err| err.into_inner());
        if let Some(parent) = self.store_path.parent() {
            fs::create_dir_all(parent).map_err(|err| {
                CaseRegistrationError::new(
                    format!("No se pudo crear el directorio de casos: {err}"),
                    "No se pudo guardar el caso en el historial local.",
                )
            })?;
        }
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.store_path)
            .map_err(|err| {
                CaseRegistrationError::new(
                    format!("No se pudo abrir el historial de casos: {err}"),
                    "No se pudo guardar el caso en el historial local.",
                )
            })?;
        let line = serde_json::to_string(record).map_err(|err| {
            CaseRegistrationError::new(err.to_string(), "No se pudo guardar el caso.")
        })?;
        writeln!(file, "{line}").map_err(|err| {
            CaseRegistrationError::new(
                format!("No se pudo escribir el caso: {err}"),
                "No se pudo guardar el caso en el historial local.",
            )
        })?;
        Ok(())
    }
}

pub fn now_colombia() -> DateTime<Tz> {
    Utc::now().with_timezone(&Bogota)
}

pub fn format_colombia(moment: DateTime<Tz>) -> String {
    moment.format("%Y-%m-%d %H:%M:%S %Z").to_string()
}

fn normalize_category(value: &str) -> String {
    let raw = value.trim().to_lowercase();
    let mapped = match raw.as_str() {
        "correo electrónico" | "correo electronico" | "correo" | "email" => "Correo electrónico",
        "dominio" => "Dominio",
        "hosting" => "Hosting",
        "facturación o compras"
        | "facturacion o compras"
        | "facturación"
        | "facturacion"
        | "compras"
        | "factura" => "Facturación o Compras",
        _ => return value.trim().to_string(),
    };
    mapped.to_string()
}

fn new_case_id(moment: DateTime<Tz>) -> String {
    let hex = Uuid::new_v4().simple().to_string();
    let suffix = hex[..4].to_uppercase();
    format!("CASO-{}-{suffix}", moment.format("%Y%m%d-%H%M%S"))
}

#[cfg(test)]
mod tests {
    use chrono::Offset;

    use super::*;

    #[test]
    fn register_case_writes_history() {
        let dir = tempfile::tempdir().unwrap();
        let store = CaseService::new(dir.path().join("casos.jsonl"));
        let record = store
            .register(
                "cliente@mi.com.co",
                "Hosting",
                "No puedo entrar al cPanel desde ayer.",
                "ses-test",
            )
            .unwrap();
        assert_eq!(record.estado, "Cerrado/Registrado");
        assert_eq!(record.correo, "cliente@mi.com.co");
        assert_eq!(record.categoria, "Hosting");
        assert!(record.case_id.starts_with("CASO-"));
        let listed = store.list_cases(50);
        assert_eq!(listed.len(), 1);
        assert!(listed[0].detalle.starts_with("No puedo entrar"));
    }

    #[test]
    fn register_case_rejects_invalid_email() {
        let dir = tempfile::tempdir().unwrap();
        let store = CaseService::new(dir.path().join("casos.jsonl"));
        let error = store
            .register("no-es-correo", "Dominio", "El dominio no abre.", "")
            .unwrap_err();
        assert!(error.user_message.contains("correo"));
    }

    #[test]
    fn register_case_normalizes_category() {
        let dir = tempfile::tempdir().unwrap();
        let store = CaseService::new(dir.path().join("casos.jsonl"));
        let record = store
            .register(
                "ana@empresa.co",
                "correo",
                "No me llegan los mensajes al buzón.",
                "",
            )
            .unwrap();
        assert_eq!(record.categoria, "Correo electrónico");
    }

    #[test]
    fn colombia_timezone_does_not_crash() {
        let stamp = now_colombia();
        assert_eq!(stamp.offset().fix().local_minus_utc(), -5 * 3600);
        assert!(!format_colombia(stamp).is_empty());
    }
}
