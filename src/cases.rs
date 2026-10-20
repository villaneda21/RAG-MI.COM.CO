//! Registro de casos de soporte en JSONL. Porta `app/services/case_service.py`.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use chrono::{DateTime, Utc};
use chrono_tz::America::Bogota;
use chrono_tz::Tz;
use regex::Regex;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::config::Settings;
use crate::models::CaseSummary;

pub const CATEGORIES: &[&str] = &[
    "Correo electrónico",
    "Dominio",
    "Hosting",
    "Facturación o Compras",
];

static EMAIL_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[^@\s]+@[^@\s]+\.[^@\s]+$").expect("email regex"));

#[derive(Debug, thiserror::Error)]
#[error("{user_message}")]
pub struct CaseRegistrationError {
    pub user_message: String,
}

impl CaseRegistrationError {
    fn new(user_message: impl Into<String>) -> Self {
        Self {
            user_message: user_message.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
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

impl From<&CaseRecord> for CaseSummary {
    fn from(record: &CaseRecord) -> Self {
        Self {
            case_id: record.case_id.clone(),
            correo: record.correo.clone(),
            categoria: record.categoria.clone(),
            detalle: record.detalle.clone(),
            registrado_en: record.registrado_en.clone(),
            estado: record.estado.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct CaseService {
    pub store_path: PathBuf,
}

impl CaseService {
    pub fn new(settings: &Settings) -> Self {
        Self {
            store_path: settings.cases_file.clone(),
        }
    }

    pub fn with_path(path: PathBuf) -> Self {
        Self { store_path: path }
    }

    pub fn register(
        &self,
        correo: &str,
        categoria: &str,
        detalle: &str,
        session_id: &str,
    ) -> Result<CaseRecord, CaseRegistrationError> {
        let email = correo.trim().to_lowercase();
        let notes = detalle.trim().to_string();
        let category = normalize_category(categoria);
        if !EMAIL_PATTERN.is_match(&email) {
            return Err(CaseRegistrationError::new(
                "El correo no parece válido. ¿Me lo confirmas de nuevo?",
            ));
        }
        if !CATEGORIES.contains(&category.as_str()) {
            return Err(CaseRegistrationError::new(
                "¿El tema es correo, dominio, hosting o facturación/compras?",
            ));
        }
        if notes.chars().count() < 3 {
            return Err(CaseRegistrationError::new(
                "Cuéntame un poco más del motivo de tu contacto para dejarlo registrado.",
            ));
        }
        let now = colombia_now();
        let record = CaseRecord {
            case_id: new_case_id(&now),
            correo: email,
            categoria: category,
            detalle: notes,
            registrado_en: now.format("%Y-%m-%d %H:%M:%S %Z").to_string(),
            estado: "Cerrado/Registrado".to_string(),
            session_id: session_id.to_string(),
        };
        self.append(&record)?;
        tracing::info!(
            case_id = %record.case_id,
            categoria = %record.categoria,
            correo = %record.correo,
            "caso registrado"
        );
        Ok(record)
    }

    pub fn list_cases(&self, limit: usize) -> Vec<CaseRecord> {
        let Ok(text) = std::fs::read_to_string(&self.store_path) else {
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
        let mut selected = records[start..].to_vec();
        selected.reverse();
        selected
    }

    fn append(&self, record: &CaseRecord) -> Result<(), CaseRegistrationError> {
        if let Some(parent) = self.store_path.parent() {
            std::fs::create_dir_all(parent).map_err(|err| {
                CaseRegistrationError::new(format!("No se pudo guardar el caso: {err}"))
            })?;
        }
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.store_path)
            .map_err(|err| {
                CaseRegistrationError::new(format!("No se pudo guardar el caso: {err}"))
            })?;
        let line = serde_json::to_string(record).map_err(|err| {
            CaseRegistrationError::new(format!("No se pudo serializar el caso: {err}"))
        })?;
        writeln!(file, "{line}").map_err(|err| {
            CaseRegistrationError::new(format!("No se pudo guardar el caso: {err}"))
        })?;
        Ok(())
    }
}

pub fn colombia_now() -> DateTime<Tz> {
    Utc::now().with_timezone(&Bogota)
}

pub fn now_stamp() -> String {
    colombia_now().format("%Y-%m-%d %H:%M:%S %Z").to_string()
}

fn new_case_id(moment: &DateTime<Tz>) -> String {
    let suffix = &Uuid::new_v4().simple().to_string()[..4];
    format!(
        "CASO-{}-{}",
        moment.format("%Y%m%d-%H%M%S"),
        suffix.to_uppercase()
    )
}

pub fn normalize_category(value: &str) -> String {
    let raw = value.trim().to_lowercase();
    match raw.as_str() {
        "correo electrónico" | "correo electronico" | "correo" | "email" => {
            "Correo electrónico".to_string()
        }
        "dominio" => "Dominio".to_string(),
        "hosting" => "Hosting".to_string(),
        "facturación o compras"
        | "facturacion o compras"
        | "facturación"
        | "facturacion"
        | "compras"
        | "factura" => "Facturación o Compras".to_string(),
        _ => value.trim().to_string(),
    }
}

pub fn ensure_parent(path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{colombia_now, CaseService};
    use std::path::PathBuf;

    fn temp_file() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("micomco-cases-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("casos.jsonl")
    }

    #[test]
    fn register_case_writes_history() {
        let path = temp_file();
        let store = CaseService::with_path(path.clone());
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
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn register_case_rejects_invalid_email() {
        let path = temp_file();
        let store = CaseService::with_path(path.clone());
        assert!(store
            .register("no-es-correo", "Dominio", "El dominio no abre.", "")
            .is_err());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn register_case_normalizes_category() {
        let path = temp_file();
        let store = CaseService::with_path(path.clone());
        let record = store
            .register(
                "ana@empresa.co",
                "correo",
                "No me llegan los mensajes al buzón.",
                "",
            )
            .unwrap();
        assert_eq!(record.categoria, "Correo electrónico");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn colombia_timezone_formats() {
        let stamp = colombia_now();
        let text = stamp.format("%Y-%m-%d %H:%M:%S %Z").to_string();
        assert!(text.contains('-') || text.contains("COT") || text.contains("05"));
    }
}
