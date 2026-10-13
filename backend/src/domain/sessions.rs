use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::domain::timeutil::bogota_stamp;

pub fn new_session_id(moment: DateTime<Utc>) -> String {
    let suffix: String = Uuid::new_v4()
        .simple()
        .to_string()
        .chars()
        .take(8)
        .collect();
    format!("ses-{}-{suffix}", bogota_stamp(moment, "%Y%m%d%H%M%S"))
}

pub fn is_safe_session_id(value: &str) -> bool {
    let raw = value.trim();
    let length = raw.chars().count();
    if !(6..=96).contains(&length) {
        return false;
    }
    raw.chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'))
}

pub fn build_summary(categoria: &str, correo: &str, messages: &[(String, String)]) -> String {
    if !categoria.is_empty() && !correo.is_empty() {
        return format!("{categoria} · {correo}");
    }
    if !categoria.is_empty() {
        return categoria.to_string();
    }
    for (role, content) in messages {
        if role != "user" {
            continue;
        }
        let text = content.split_whitespace().collect::<Vec<_>>().join(" ");
        if text.is_empty() {
            continue;
        }
        let mut short: String = text.chars().take(140).collect();
        if text.chars().count() > 140 {
            short.push('…');
        }
        return short;
    }
    "Conversación de soporte".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn rejects_path_traversal_and_short_ids() {
        assert!(!is_safe_session_id("../etc/passwd"));
        assert!(!is_safe_session_id("abc"));
        assert!(!is_safe_session_id("sesion con espacios"));
        assert!(is_safe_session_id("ses-20260715120000-abcd1234"));
    }

    #[test]
    fn session_id_uses_bogota_clock() {
        let moment = Utc
            .with_ymd_and_hms(2026, 7, 15, 17, 0, 0)
            .single()
            .expect("fecha válida");
        let session_id = new_session_id(moment);
        assert!(session_id.starts_with("ses-20260715120000-"));
    }

    #[test]
    fn summary_prefers_case_fields() {
        let summary = build_summary("Dominio", "a@b.co", &[("user".into(), "hola".into())]);
        assert_eq!(summary, "Dominio · a@b.co");
    }
}
