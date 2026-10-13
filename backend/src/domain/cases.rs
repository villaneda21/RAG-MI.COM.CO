use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::domain::text::fold_search;
use crate::domain::timeutil::bogota_stamp;

pub const CATEGORIES: [&str; 4] = [
    "Correo electrónico",
    "Dominio",
    "Hosting",
    "Facturación o Compras",
];

const ALIASES: &[(&str, &str)] = &[
    ("facturacion o compras", "Facturación o Compras"),
    ("correo electronico", "Correo electrónico"),
    ("facturacion", "Facturación o Compras"),
    ("compras", "Facturación o Compras"),
    ("factura", "Facturación o Compras"),
    ("correo", "Correo electrónico"),
    ("email", "Correo electrónico"),
    ("dominio", "Dominio"),
    ("hosting", "Hosting"),
];

pub const CASE_STATUS: &str = "Cerrado/Registrado";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedCase {
    pub correo: String,
    pub categoria: String,
    pub detalle: String,
}

pub fn new_case_id(moment: DateTime<Utc>) -> String {
    let suffix: String = Uuid::new_v4()
        .simple()
        .to_string()
        .chars()
        .take(4)
        .collect::<String>()
        .to_uppercase();
    format!("CASO-{}-{suffix}", bogota_stamp(moment, "%Y%m%d-%H%M%S"))
}

pub fn normalize_category(value: &str) -> String {
    let folded = fold_search(value);
    for (alias, canonical) in ALIASES {
        if folded == *alias {
            return (*canonical).to_string();
        }
    }
    for canonical in CATEGORIES {
        if fold_search(canonical) == folded {
            return canonical.to_string();
        }
    }
    value.trim().to_string()
}

pub fn extract_category(text: &str) -> Option<&'static str> {
    let padded = format!(" {} ", fold_search(text));
    let mut best: Option<(usize, usize, &'static str)> = None;
    for (alias, canonical) in ALIASES {
        let needle = format!(" {alias} ");
        if let Some(position) = padded.rfind(&needle) {
            let length = alias.len();
            let replace = match best {
                None => true,
                Some((best_pos, best_len, _)) => {
                    position > best_pos || (position == best_pos && length > best_len)
                }
            };
            if replace {
                best = Some((position, length, canonical));
            }
        }
    }
    best.map(|(_, _, canonical)| canonical)
}

pub fn is_valid_email(value: &str) -> bool {
    let email = value.trim();
    if email.len() < 3 || email.len() > 320 || email.chars().any(char::is_whitespace) {
        return false;
    }
    let mut parts = email.split('@');
    let Some(local) = parts.next() else {
        return false;
    };
    let Some(domain) = parts.next() else {
        return false;
    };
    if parts.next().is_some() || local.is_empty() || domain.is_empty() {
        return false;
    }
    match domain.rsplit_once('.') {
        Some((name, tld)) => !name.is_empty() && !tld.is_empty(),
        None => false,
    }
}

pub fn find_emails(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    for token in text.split_whitespace() {
        let cleaned = token.trim_matches(|ch: char| {
            matches!(
                ch,
                ',' | ';' | ':' | '.' | '(' | ')' | '"' | '\'' | '<' | '>' | '[' | ']'
            )
        });
        if is_valid_email(cleaned) {
            found.push(cleaned.to_lowercase());
        }
    }
    found
}

pub fn validate_case(
    correo: &str,
    categoria: &str,
    detalle: &str,
) -> Result<ValidatedCase, String> {
    let email = correo.trim().to_lowercase();
    if !is_valid_email(&email) {
        return Err("El correo no parece válido. ¿Me lo confirmas de nuevo?".into());
    }
    let category = normalize_category(categoria);
    if !CATEGORIES.contains(&category.as_str()) {
        return Err("¿El tema es correo, dominio, hosting o facturación/compras?".into());
    }
    let notes = detalle.split_whitespace().collect::<Vec<_>>().join(" ");
    let notes: String = notes.chars().take(2000).collect();
    if notes.chars().count() < 3 {
        return Err(
            "Cuéntame un poco más del motivo de tu contacto para dejarlo registrado.".into(),
        );
    }
    Ok(ValidatedCase {
        correo: email,
        categoria: category,
        detalle: notes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn normalizes_aliases_and_rejects_unknown_categories() {
        assert_eq!(normalize_category("correo"), "Correo electrónico");
        assert_eq!(normalize_category("Facturación"), "Facturación o Compras");
        assert!(validate_case("no-es-correo", "dominio", "detalle suficiente").is_err());
        assert!(validate_case("a@b.co", "otro", "detalle suficiente").is_err());
        assert!(validate_case("a@b.co", "dominio", "ok").is_err());
        let draft =
            validate_case("Cliente@MI.com.co", "hosting", "el sitio no abre").expect("caso válido");
        assert_eq!(draft.correo, "cliente@mi.com.co");
        assert_eq!(draft.categoria, "Hosting");
    }

    #[test]
    fn case_id_uses_bogota_local_time() {
        let moment = Utc
            .with_ymd_and_hms(2026, 7, 15, 17, 0, 0)
            .single()
            .expect("fecha válida");
        let case_id = new_case_id(moment);
        assert!(
            case_id.starts_with("CASO-20260715-120000-"),
            "identificador inesperado: {case_id}"
        );
    }

    #[test]
    fn extracts_the_latest_category() {
        let category = extract_category("primero el correo y después el dominio");
        assert_eq!(category, Some("Dominio"));
    }
}
