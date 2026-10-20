//! Clasificación, filtro de medios y limpieza de respuestas al cliente.

use std::sync::LazyLock;

use regex::Regex;

pub const DEPT_SUPPORT: &str = "Soporte Técnico";
pub const DEPT_SALES: &str = "Ventas";
pub const DEPT_BILLING: &str = "Facturación";

pub const SATISFACTION_QUESTION: &str =
    "¿Pude resolver tu duda o hay algo más en lo que te pueda ayudar?";

pub const RESOLVED_NOTICE: &str = "✅ Tu caso ha sido marcado como RESUELTO. Si necesitas ayuda adicional o tu problema no se solucionó, escríbenos de nuevo.";

pub fn transfer_notice(area: &str) -> String {
    format!("🔄 Tu caso ha sido transferido al área de {area}. Un agente te atenderá en breve.")
}

pub fn agent_notice(name: &str) -> String {
    format!("👤 El agente {name} ha asumido tu conversación y te está atendiendo ahora.")
}

pub fn handoff_notice(label: &str, area: &str) -> String {
    format!(
        "🔄 Entendido. He transferido tu caso ({label}) al equipo de {area}. Un agente humano te atenderá en breve."
    )
}

pub const COVERAGE_MARK: &str = "[[SIN_COBERTURA]]";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandoffKind {
    Human,
    Knowledge,
}

pub fn reopen_notice(label: &str) -> String {
    format!("ℹ️ Tu caso anterior ya se encuentra cerrado. Hemos generado una nueva solicitud ({label}) para atender tu mensaje.")
}

pub fn ticket_label(id: uuid::Uuid) -> String {
    let compact = id.simple().to_string();
    format!("#{}", compact[..8].to_uppercase())
}

pub const TIMEOUT_STATUSES: &[&str] = &["en_bot", "esperando_agente", "en_proceso"];

pub const DEFAULT_WARNING_MINUTES: i32 = 10;
pub const DEFAULT_CLOSE_MINUTES: i32 = 5;
pub const DEFAULT_WARNING_TEMPLATE: &str =
    "Seguimos por aquí. Si no recibimos respuesta en los próximos minutos, cerraremos esta conversación.";
pub const DEFAULT_CLOSE_TEMPLATE: &str =
    "Cerramos este chat por inactividad. Cuando quieras, escríbenos de nuevo y te ayudamos.";

const ROUTES: &[(&str, &str)] = &[
    ("factura", DEPT_BILLING),
    ("facturacion", DEPT_BILLING),
    ("datos", DEPT_BILLING),
    ("cotizacion", DEPT_SALES),
    ("precio", DEPT_SALES),
    ("reembolso", DEPT_SUPPORT),
    ("falla", DEPT_SUPPORT),
];

const CONFIRMATIONS: &[&str] = &[
    "si",
    "sip",
    "ok",
    "okay",
    "dale",
    "listo",
    "gracias",
    "muchas gracias",
    "ok gracias",
    "si gracias",
    "perfecto",
    "resuelto",
    "resuelta",
    "quedo resuelto",
    "quedo resuelta",
    "ya quedo",
    "nada mas",
    "eso es todo",
    "no gracias",
    "todo bien",
    "me sirvio",
    "era eso",
    "esa era",
    "no necesito nada mas",
    "esta bien",
];

static MD_LINK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\[([^\]\n]+)\]\((?:https?://|www\.)[^)\s]+\)").expect("regex de enlaces")
});
static URL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b(?:https?://|www\.)\S+").expect("regex de urls"));
static PAREN_SOURCE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\s*[\(\[](?:fuente|fuentes|source|sources|ref|referencia|ver)[^)\]]*[\)\]]")
        .expect("regex de citas")
});
static CHUNK_HEADER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\[(?:chunk|fuente)[^\]]*\]").expect("regex de chunks"));

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaDecision {
    Text,
    Audio { extension: &'static str },
    RejectStickers,
    RejectOther,
}

pub fn fold(text: &str) -> String {
    text.chars()
        .map(|character| match character {
            'á' | 'à' | 'ä' | 'â' => 'a',
            'é' | 'è' | 'ë' | 'ê' => 'e',
            'í' | 'ì' | 'ï' | 'î' => 'i',
            'ó' | 'ò' | 'ö' | 'ô' => 'o',
            'ú' | 'ù' | 'ü' | 'û' => 'u',
            'ñ' => 'n',
            other => other.to_ascii_lowercase(),
        })
        .collect()
}

fn term_at(haystack: &str, term: &str) -> Option<usize> {
    let mut rest = haystack;
    let mut offset = 0usize;
    while let Some(found) = rest.find(term) {
        let absolute = offset + found;
        let before_ok = absolute == 0
            || !haystack
                .chars()
                .nth(absolute.saturating_sub(1))
                .is_some_and(|c| c.is_ascii_alphanumeric());
        let after = absolute + term.len();
        let after_ok = !haystack
            .chars()
            .nth(after)
            .is_some_and(|c| c.is_ascii_alphanumeric());
        if before_ok && after_ok {
            return Some(absolute);
        }
        let next = found + term.len();
        offset += next;
        rest = &rest[next..];
    }
    None
}

pub fn matched_department(text: &str) -> Option<&'static str> {
    let folded = fold(text);
    let mut best: Option<(usize, &'static str)> = None;
    for (term, department) in ROUTES {
        if let Some(position) = term_at(&folded, term) {
            let replace = best.is_none_or(|(current, _)| position < current);
            if replace {
                best = Some((position, department));
            }
        }
    }
    best.map(|(_, department)| department)
}

pub fn route_department(text: &str) -> &'static str {
    matched_department(text).unwrap_or(DEPT_SUPPORT)
}

pub fn handoff_area(kind: HandoffKind, text: &str, current: &str) -> &'static str {
    if let Some(area) = matched_department(text) {
        return area;
    }
    if fold(text).contains("soporte") {
        return DEPT_SUPPORT;
    }
    match kind {
        HandoffKind::Knowledge => DEPT_SUPPORT,
        HandoffKind::Human => match current {
            DEPT_SALES => DEPT_SALES,
            DEPT_BILLING => DEPT_BILLING,
            _ => DEPT_SUPPORT,
        },
    }
}

pub fn priority_for(text: &str) -> &'static str {
    let folded = fold(text);
    if term_at(&folded, "reembolso").is_some() || term_at(&folded, "falla").is_some() {
        "alta"
    } else {
        "normal"
    }
}

pub fn requests_human(text: &str) -> bool {
    let folded = fold(text);
    if ["agente", "asesor", "asesora", "humano", "operador"]
        .iter()
        .any(|term| term_at(&folded, term).is_some())
        || folded.contains("persona real")
        || folded.contains("hablar con alguien")
    {
        return true;
    }
    let asks_person = [
        "comunicame",
        "comuniqueme",
        "pasame",
        "conectame",
        "transfiereme",
    ]
    .iter()
    .any(|term| folded.contains(term));
    asks_person
        && (folded.contains("soporte")
            || folded.contains("asesor")
            || folded.contains("agente")
            || folded.contains("humano")
            || folded.contains("persona"))
}

pub fn lacks_knowledge(answer: &str) -> bool {
    if answer.contains(COVERAGE_MARK) {
        return true;
    }
    let folded = fold(answer);
    const MARKERS: &[&str] = &[
        "no encontre informacion",
        "no tengo informacion suficiente",
        "no tengo informacion sobre",
        "no esta en la base de conocimiento",
        "no aparece en la documentacion",
        "no aparece en la base de conocimiento",
        "la base de conocimiento no",
        "no cubre el tema",
        "no hay informacion suficiente",
        "sin informacion suficiente",
    ];
    MARKERS.iter().any(|marker| folded.contains(marker))
}

pub fn client_confirms_resolution(text: &str) -> bool {
    if text.chars().count() > 60 || text.contains('?') || text.contains('¿') {
        return false;
    }
    let folded = fold(text);
    if folded.contains("pero") || folded.contains("tambien") || folded.contains("otra") {
        return false;
    }
    let compact = folded
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character.is_whitespace() {
                character
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    CONFIRMATIONS.contains(&compact.as_str())
}

pub fn media_decision(mime: &str, filename: &str) -> MediaDecision {
    let mime = mime
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let name = filename.trim().to_ascii_lowercase();
    let sticker = mime == "image/webp"
        || mime == "image/gif"
        || name.ends_with(".gif")
        || name.ends_with(".webp");
    if sticker {
        return MediaDecision::RejectStickers;
    }
    let audio = match mime.as_str() {
        "audio/ogg" | "audio/opus" => Some("ogg"),
        "audio/mpeg" | "audio/mp3" => Some("mp3"),
        "audio/wav" | "audio/x-wav" | "audio/wave" => Some("wav"),
        _ => None,
    };
    if let Some(extension) = audio {
        return MediaDecision::Audio { extension };
    }
    if name.ends_with(".ogg") {
        return MediaDecision::Audio { extension: "ogg" };
    }
    if name.ends_with(".mp3") {
        return MediaDecision::Audio { extension: "mp3" };
    }
    if name.ends_with(".wav") {
        return MediaDecision::Audio { extension: "wav" };
    }
    if mime.starts_with("image/") || mime.starts_with("video/") || !name.is_empty() {
        return MediaDecision::RejectOther;
    }
    MediaDecision::Text
}

pub fn strip_sources(answer: &str) -> String {
    let without_chunks = CHUNK_HEADER.replace_all(answer, "");
    let without_links = MD_LINK.replace_all(&without_chunks, "$1");
    let without_urls = URL_RE.replace_all(&without_links, "");
    let without_cites = PAREN_SOURCE.replace_all(&without_urls, "");
    let mut kept = Vec::new();
    let mut skipping_list = false;
    for line in without_cites.lines() {
        let trimmed = line.trim();
        if is_source_heading(trimmed) {
            skipping_list = true;
            continue;
        }
        if skipping_list {
            if trimmed.is_empty() {
                skipping_list = false;
            }
            continue;
        }
        if is_citation_line(trimmed) {
            continue;
        }
        kept.push(line.trim_end());
    }
    let mut collapsed = String::new();
    let mut blank = 0u8;
    for line in kept {
        if line.trim().is_empty() {
            blank = blank.saturating_add(1);
            if blank <= 1 && !collapsed.is_empty() {
                collapsed.push('\n');
            }
            continue;
        }
        blank = 0;
        if !collapsed.is_empty() {
            collapsed.push('\n');
        }
        collapsed.push_str(line.trim());
    }
    collapsed.trim().to_string()
}

fn is_source_heading(line: &str) -> bool {
    let cleaned = line
        .trim_matches('#')
        .trim()
        .trim_end_matches(':')
        .trim()
        .to_lowercase();
    matches!(
        cleaned.as_str(),
        "fuentes"
            | "fuente"
            | "referencias"
            | "referencia"
            | "references"
            | "sources"
            | "bibliografia"
            | "bibliografía"
    )
}

fn is_citation_line(line: &str) -> bool {
    let lower = line.to_lowercase();
    lower.starts_with("fuente:")
        || lower.starts_with("fuentes:")
        || lower.starts_with("source:")
        || lower.starts_with("sources:")
        || lower.starts_with("referencia:")
        || lower.starts_with("ver también:")
        || lower.starts_with("ver tambien:")
}

pub fn visible_reply(answer: &str) -> String {
    strip_sources(answer)
        .replace(COVERAGE_MARK, "")
        .trim()
        .to_string()
}

pub fn normalize_shortcut(raw: &str) -> Option<String> {
    let trimmed = raw.trim().to_ascii_lowercase();
    let with_slash = if trimmed.starts_with('/') {
        trimmed
    } else {
        format!("/{trimmed}")
    };
    if with_slash.len() < 2 || with_slash.len() > 33 {
        return None;
    }
    if !with_slash.chars().enumerate().all(|(index, character)| {
        if index == 0 {
            character == '/'
        } else {
            character.is_ascii_alphanumeric() || character == '_' || character == '-'
        }
    }) {
        return None;
    }
    Some(with_slash)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_keywords_to_the_first_department() {
        assert_eq!(route_department("hay una falla en el correo"), DEPT_SUPPORT);
        assert_eq!(route_department("quiero un reembolso"), DEPT_SUPPORT);
        assert_eq!(
            route_department("me pasan el precio del hosting"),
            DEPT_SALES
        );
        assert_eq!(
            route_department("necesito una cotización anual"),
            DEPT_SALES
        );
        assert_eq!(
            route_department("enviame la factura de marzo"),
            DEPT_BILLING
        );
        assert_eq!(
            route_department("actualicen mis datos de pago"),
            DEPT_BILLING
        );
        assert_eq!(route_department("hola, no carga mi sitio"), DEPT_SUPPORT);
        assert_eq!(priority_for("falla de dns"), "alta");
        assert_eq!(priority_for("precio del plan"), "normal");
    }

    #[test]
    fn confirmation_is_short_and_affirmative() {
        assert!(client_confirms_resolution("Sí, gracias"));
        assert!(client_confirms_resolution("nada más"));
        assert!(!client_confirms_resolution(
            "sí, pero sigue caído el correo"
        ));
        assert!(!client_confirms_resolution("¿ya quedó?"));
        assert!(requests_human("quiero hablar con un agente"));
        assert!(requests_human("comunícame con soporte"));
        assert!(requests_human("pásame con un asesor"));
        assert!(!requests_human("el soporte del correo no abre"));
        assert!(lacks_knowledge(
            "[[SIN_COBERTURA]] No está en la base de conocimiento."
        ));
        assert!(lacks_knowledge(
            "No encontré información suficiente sobre esta pregunta en la base de conocimiento disponible."
        ));
        assert!(!lacks_knowledge(
            "Entra a Mis servicios y pulsa Acceder a cPanel."
        ));
        assert_eq!(
            handoff_area(
                HandoffKind::Knowledge,
                "certificado SSL en cPanel",
                DEPT_BILLING
            ),
            DEPT_SUPPORT
        );
        assert_eq!(
            handoff_area(HandoffKind::Human, "quiero un agente", DEPT_BILLING),
            DEPT_BILLING
        );
        assert!(handoff_notice("#AB12CD34", DEPT_SUPPORT).contains("Soporte Técnico"));
    }

    #[test]
    fn strips_bibliographic_sources() {
        let raw = "El código está en Mi cuenta [guía](https://mi.com.co/cuenta).\n\
Consulta https://docs.example.com/cli para el formato.\n\
\n\
Fuentes:\n\
- documento.txt\n\
\n\
Fuente: documento.txt";
        let clean = strip_sources(raw);
        assert!(!clean.to_lowercase().contains("http"));
        assert!(!clean.to_lowercase().contains("fuentes"));
        assert!(!clean.to_lowercase().contains("documento.txt"));
        assert!(clean.contains("Mi cuenta"));
        let asked = visible_reply(raw);
        assert!(!asked.contains(SATISFACTION_QUESTION));
        assert!(!asked.contains("https://"));
    }

    #[test]
    fn notices_name_the_area_agent_and_new_request() {
        assert!(RESOLVED_NOTICE.contains("RESUELTO"));
        assert!(transfer_notice("Facturación").contains("área de Facturación"));
        assert!(agent_notice("Agente Ventas").contains("El agente Agente Ventas"));
        let id = uuid::Uuid::parse_str("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa1").unwrap();
        let label = ticket_label(id);
        assert_eq!(label, "#AAAAAAAA");
        assert!(reopen_notice(&label).contains("#AAAAAAAA"));
        assert!(reopen_notice(&label).contains("nueva solicitud"));
    }

    #[test]
    fn rejects_stickers_and_accepts_voice_notes() {
        assert_eq!(
            media_decision("image/webp", "sticker.webp"),
            MediaDecision::RejectStickers
        );
        assert_eq!(
            media_decision("image/gif", ""),
            MediaDecision::RejectStickers
        );
        assert_eq!(
            media_decision("audio/ogg", "nota.ogg"),
            MediaDecision::Audio { extension: "ogg" }
        );
        assert_eq!(
            media_decision("audio/mp3", ""),
            MediaDecision::Audio { extension: "mp3" }
        );
        assert_eq!(
            media_decision("audio/wav", "voz.wav"),
            MediaDecision::Audio { extension: "wav" }
        );
        assert_eq!(media_decision("", ""), MediaDecision::Text);
        assert_eq!(
            media_decision("image/png", "foto.png"),
            MediaDecision::RejectOther
        );
    }
}
