use crate::domain::cases::{
    extract_category, find_emails, validate_case, ValidatedCase, CATEGORIES,
};
use crate::domain::models::{ChatMessage, RetrievedChunk};
use crate::domain::text::{cap_chars, fold_search};

const CONFIRM_MARKER: &str = "responde CONFIRMO";
const COLLECT_MARKER: &str = "Puedo dejar el caso registrado";

const EXPLICIT_PHRASES: &[&str] = &[
    "registrar un caso",
    "registrar el caso",
    "dejar un caso",
    "abrir un caso",
    "quiero un asesor",
    "hablar con un asesor",
    "hablar con asesor",
    "necesito que lo gestionen",
    "radicar un caso",
    "radicar caso",
    "quiero registrar",
];

#[derive(Debug, PartialEq, Eq)]
pub enum Classification {
    Ask(String),
    Register(ValidatedCase),
    Knowledge { diagnostic: bool, offer_case: bool },
}

pub fn classify_turn(question: &str, history: &[ChatMessage]) -> Classification {
    let folded = fold_search(question);
    if !case_controls(question, &folded, history) {
        return Classification::Knowledge {
            diagnostic: is_diagnostic(&folded),
            offer_case: sounds_like_problem(&folded),
        };
    }

    let stage = assistant_stage(history);
    if stage == CaseStage::Confirm && is_negative(&folded) {
        return Classification::Ask(
            "De acuerdo, no registré el caso. Cuando quieras, dime el dato correcto o vuelve a pedir el registro.".into(),
        );
    }

    let texts = relevant_user_texts(history, question);
    let email = last_email(&texts);
    let category = last_category(&texts);
    let detail = collect_detail(&texts);

    let Some(correo) = email else {
        return Classification::Ask(ask_missing(None, category, detail.chars().count() >= 3));
    };
    let Some(categoria) = category else {
        return Classification::Ask(ask_missing(
            Some(&correo),
            None,
            detail.chars().count() >= 3,
        ));
    };
    if detail.chars().count() < 3 {
        return Classification::Ask(ask_missing(Some(&correo), Some(categoria), false));
    }

    let confirmed =
        is_confirmation(&folded) && (stage == CaseStage::Confirm || contains_explicit(&folded));
    if !confirmed {
        return Classification::Ask(format!(
            "Voy a registrar el caso con estos datos:\n- Correo: {correo}\n- Categoría: {categoria}\n- Detalle: {detail}\nSi son correctos, responde CONFIRMO. Si hay que corregir algo, escríbelo antes de confirmar."
        ));
    }

    match validate_case(&correo, categoria, &detail) {
        Ok(draft) => Classification::Register(draft),
        Err(message) => Classification::Ask(message),
    }
}

pub fn farewell(
    case_id: &str,
    estado: &str,
    correo: &str,
    categoria: &str,
    detalle: &str,
) -> String {
    format!(
        "Listo. Dejé registrado el caso {case_id} con estado {estado}.\n- Correo: {correo}\n- Categoría: {categoria}\n- Detalle: {detalle}\nEl equipo de soporte de MI.COM.CO continuará la gestión con esta información."
    )
}

pub fn select_hits(hits: &[RetrievedChunk]) -> Vec<&RetrievedChunk> {
    let mut ordered: Vec<&RetrievedChunk> = hits.iter().filter(|hit| hit.score > 0.0).collect();
    ordered.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let Some(best) = ordered.first().map(|hit| hit.score) else {
        return Vec::new();
    };
    ordered
        .into_iter()
        .take(3)
        .filter(|hit| hit.score >= best * 0.25)
        .collect()
}

pub fn render_knowledge(diagnostic: bool, offer_case: bool, hits: &[&RetrievedChunk]) -> String {
    let mut out = String::new();
    if diagnostic {
        out.push_str(
            "No puedo revisar en vivo DNS, SPF, DKIM, DMARC, ping ni el estado de los servidores. Sí puedo orientarte con el procedimiento documentado.\n\n",
        );
    }
    if hits.is_empty() {
        out.push_str(
            "No encontré información suficiente sobre esta pregunta en la base de conocimiento disponible.",
        );
        out.push_str("\n\nSi necesitas gestión de la cuenta, dime que quieres registrar un caso.");
        return cap_chars(&out, 12_000);
    }

    out.push_str("Según la base de conocimiento de MI.COM.CO:");
    let mut ids = Vec::new();
    for hit in hits {
        out.push_str("\n\n");
        if !hit.title.is_empty() {
            out.push_str(&hit.title);
            out.push('\n');
        }
        if let Some(id) = &hit.doc_chunk_id {
            out.push_str("Identificador: ");
            out.push_str(id);
            if let Some(part) = hit.doc_chunk_part {
                out.push_str(" (parte ");
                out.push_str(&part.to_string());
                out.push(')');
            }
            out.push('\n');
            ids.push(id.clone());
        }
        if !hit.category.is_empty() {
            out.push_str("Categoría: ");
            out.push_str(&hit.category);
            out.push('\n');
        }
        if !hit.summary.is_empty() {
            out.push_str(&hit.summary);
            out.push_str("\n\n");
        }
        out.push_str(&cap_chars(&hit.content, 4_000));
    }
    if !ids.is_empty() {
        out.push_str("\n\nFuentes consultadas: ");
        out.push_str(&ids.join(", "));
        out.push('.');
    }
    if offer_case {
        out.push_str("\n\nSi con esto no puedes avanzar, dime que quieres registrar un caso.");
    }
    cap_chars(&out, 12_000)
}

pub fn build_retrieval_query(question: &str, history: &[ChatMessage]) -> String {
    let window: Vec<&ChatMessage> = history.iter().rev().take(6).collect();
    let mut parts: Vec<String> = Vec::new();
    for item in window.into_iter().rev() {
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
    let mut deduped: Vec<String> = Vec::new();
    for part in parts {
        if deduped.last().map(String::as_str) != Some(part.as_str()) {
            deduped.push(part);
        }
    }
    let start = deduped.len().saturating_sub(3);
    deduped[start..].join("\n")
}

pub fn validate_question(raw: &str) -> Result<String, String> {
    let cleaned = raw.trim();
    if cleaned.is_empty() {
        return Err("La pregunta no puede estar vacía.".into());
    }
    if cleaned.chars().count() > 2_000 {
        return Err("La pregunta es demasiado larga (máximo 2000 caracteres).".into());
    }
    Ok(cleaned.to_string())
}

pub fn sanitize_history(history: &[ChatMessage]) -> Vec<ChatMessage> {
    history
        .iter()
        .filter_map(|message| {
            let role = message.role.trim();
            if role != "user" && role != "assistant" {
                return None;
            }
            let content = message.content.trim();
            if content.is_empty() {
                return None;
            }
            Some(ChatMessage {
                role: role.to_string(),
                content: cap_chars(content, 8_000),
            })
        })
        .collect()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CaseStage {
    None,
    Collect,
    Confirm,
}

fn assistant_stage(history: &[ChatMessage]) -> CaseStage {
    let Some(last) = history
        .iter()
        .rev()
        .find(|message| message.role == "assistant")
    else {
        return CaseStage::None;
    };
    if last.content.contains(CONFIRM_MARKER) {
        CaseStage::Confirm
    } else if last.content.contains(COLLECT_MARKER) {
        CaseStage::Collect
    } else {
        CaseStage::None
    }
}

fn case_controls(question: &str, folded: &str, history: &[ChatMessage]) -> bool {
    if contains_explicit(folded) {
        return true;
    }
    match assistant_stage(history) {
        CaseStage::None => false,
        CaseStage::Collect | CaseStage::Confirm => {
            is_confirmation(folded)
                || is_negative(folded)
                || !find_emails(question).is_empty()
                || is_category_reply(question)
                || folded.chars().count() < 280
        }
    }
}

fn contains_explicit(folded: &str) -> bool {
    EXPLICIT_PHRASES
        .iter()
        .any(|phrase| folded.contains(phrase))
}

fn is_diagnostic(folded: &str) -> bool {
    let action = [
        "revisa",
        "revisar",
        "verifica",
        "verificar",
        "comprueba",
        "comprobar",
        "haz ping",
        "hacer ping",
        "estado del servidor",
        "estado de la red",
    ]
    .iter()
    .any(|phrase| folded.contains(phrase));
    let target = [
        "dns", "spf", "dkim", "dmarc", "ping", "servidor", "la ip", "mi ip",
    ]
    .iter()
    .any(|phrase| folded.contains(phrase));
    action && target
}

fn sounds_like_problem(folded: &str) -> bool {
    [
        "no puedo",
        "no me deja",
        "no funciona",
        "no tengo acceso",
        "se bloque",
        " error",
        "error ",
    ]
    .iter()
    .any(|phrase| folded.contains(phrase) || folded.starts_with(phrase.trim()))
}

fn is_bare_confirmation(folded: &str) -> bool {
    matches!(
        folded,
        "si" | "ok"
            | "okay"
            | "dale"
            | "correcto"
            | "de acuerdo"
            | "adelante"
            | "confirmo"
            | "confirmado"
            | "registralo"
    )
}

fn is_confirmation(folded: &str) -> bool {
    if is_bare_confirmation(folded) {
        return true;
    }
    let tokens: Vec<&str> = folded.split_whitespace().collect();
    let has_confirm = tokens
        .iter()
        .any(|token| matches!(*token, "confirmo" | "confirmado" | "registralo"));
    has_confirm && (tokens.len() <= 6 || contains_explicit(folded))
}

fn is_negative(folded: &str) -> bool {
    matches!(
        folded,
        "no" | "cancelar" | "corregir" | "no gracias" | "cancela" | "cancelar el caso"
    )
}

fn is_category_reply(question: &str) -> bool {
    extract_category(question).is_some() && fold_search(question).split_whitespace().count() <= 4
}

fn relevant_user_texts<'a>(history: &'a [ChatMessage], question: &'a str) -> Vec<&'a str> {
    let mut texts = Vec::new();
    if let Some(index) = history.iter().rposition(|message| {
        message.role == "user" && contains_explicit(&fold_search(&message.content))
    }) {
        texts.extend(
            history[index..]
                .iter()
                .filter(|message| message.role == "user")
                .map(|message| message.content.as_str()),
        );
    }
    texts.push(question);
    texts
}

fn last_email(texts: &[&str]) -> Option<String> {
    let mut found = None;
    for text in texts {
        if let Some(email) = find_emails(text).into_iter().last() {
            found = Some(email);
        }
    }
    found
}

fn last_category(texts: &[&str]) -> Option<&'static str> {
    let mut found = None;
    for text in texts {
        if let Some(category) = extract_category(text) {
            found = Some(category);
        }
    }
    found
}

fn collect_detail(texts: &[&str]) -> String {
    let mut parts = Vec::new();
    for text in texts {
        if let Some(detail) = meaningful_detail(text) {
            parts.push(detail);
        }
    }
    let joined = parts.join(" ");
    let collapsed = joined.split_whitespace().collect::<Vec<_>>().join(" ");
    cap_chars(&collapsed, 2_000)
}

fn meaningful_detail(message: &str) -> Option<String> {
    let folded = fold_search(message);
    if folded.is_empty() || is_bare_confirmation(&folded) || is_negative(&folded) {
        return None;
    }
    if EXPLICIT_PHRASES.iter().any(|phrase| folded == *phrase) {
        return None;
    }
    if is_email_only(message) || is_category_only(message) {
        return None;
    }
    Some(message.trim().to_string())
}

fn is_email_only(message: &str) -> bool {
    let emails = find_emails(message);
    if emails.is_empty() {
        return false;
    }
    let mut rest = message.to_string();
    for email in &emails {
        rest = remove_ignore_ascii_case(&rest, email);
    }
    fold_search(&rest).split_whitespace().all(|token| {
        matches!(
            token,
            "correo" | "email" | "es" | "mi" | "el" | "la" | "de" | "registrado"
        )
    })
}

fn is_category_only(message: &str) -> bool {
    let Some(category) = extract_category(message) else {
        return false;
    };
    let mut rest = fold_search(message).replace(&fold_search(category), " ");
    for stop in [
        "categoria",
        "es",
        "de",
        "el",
        "la",
        "los",
        "las",
        "un",
        "una",
        "mi",
        "tema",
        "sobre",
    ] {
        rest = format!(" {} ", rest).replace(&format!(" {stop} "), " ");
    }
    fold_search(&rest).is_empty()
}

fn remove_ignore_ascii_case(haystack: &str, needle: &str) -> String {
    if needle.is_empty() {
        return haystack.to_string();
    }
    let hay: Vec<char> = haystack.chars().collect();
    let needle: Vec<char> = needle.chars().flat_map(|ch| ch.to_lowercase()).collect();
    let mut index = 0;
    let mut out = String::new();
    while index < hay.len() {
        if starts_with_ignore_case(&hay[index..], &needle) {
            index += needle.len();
            continue;
        }
        out.push(hay[index]);
        index += 1;
    }
    out
}

fn starts_with_ignore_case(hay: &[char], needle: &[char]) -> bool {
    if hay.len() < needle.len() {
        return false;
    }
    hay.iter()
        .zip(needle.iter())
        .all(|(left, right)| left.to_lowercase().next().as_ref() == Some(right))
}

fn ask_missing(email: Option<&str>, category: Option<&str>, detail_ok: bool) -> String {
    let mut missing = Vec::new();
    if email.is_none() {
        missing.push("el correo con el que estás registrado en MI.COM.CO".to_string());
    }
    if category.is_none() {
        missing.push(format!("la categoría ({})", CATEGORIES.join(", ")));
    }
    if !detail_ok {
        missing.push("un breve detalle de lo que ocurre".to_string());
    }
    if missing.len() == 1 {
        format!("Puedo dejar el caso registrado. Me falta {}.", missing[0])
    } else {
        format!(
            "Puedo dejar el caso registrado. Me faltan: {}.",
            missing.join("; ")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit() -> RetrievedChunk {
        RetrievedChunk {
            chunk_index: 3,
            doc_chunk_id: Some("GUA-003".into()),
            doc_chunk_part: None,
            title: "Cómo cambiar la contraseña de tu cuenta y activar 2FA".into(),
            category: "Guías Operativas - Seguridad de Cuenta".into(),
            entity: "MI.COM.CO".into(),
            keywords: vec!["2FA".into()],
            summary: "Instrucciones para cambiar la contraseña.".into(),
            content:
                "Longitud mínima de 10 caracteres. Activa la Autenticación de Dos Pasos (2FA)."
                    .into(),
            source: "base.txt".into(),
            score: 1.2,
        }
    }

    #[test]
    fn knowledge_question_does_not_open_a_case() {
        let classification = classify_turn("¿Cómo cambio la contraseña?", &[]);
        assert_eq!(
            classification,
            Classification::Knowledge {
                diagnostic: false,
                offer_case: false
            }
        );
        let chunk = hit();
        let answer = render_knowledge(false, false, &[&chunk]);
        assert!(answer.contains("2FA"));
        assert!(answer.contains("GUA-003"));
        assert!(!answer.contains("Listo. Dejé registrado"));
    }

    #[test]
    fn refuses_live_diagnostics() {
        let classification = classify_turn("verifica el SPF de mi dominio ahora", &[]);
        assert_eq!(
            classification,
            Classification::Knowledge {
                diagnostic: true,
                offer_case: false
            }
        );
        let answer = render_knowledge(true, false, &[]);
        assert!(answer.contains("No puedo revisar en vivo"));
        assert!(answer.contains("No encontré información suficiente"));
    }

    #[test]
    fn case_flow_asks_then_confirms_then_registers() {
        let first = classify_turn("quiero registrar un caso", &[]);
        let Classification::Ask(prompt) = first else {
            panic!("debía pedir datos");
        };
        assert!(prompt.contains(COLLECT_MARKER));

        let history = vec![
            ChatMessage {
                role: "user".into(),
                content: "quiero registrar un caso".into(),
            },
            ChatMessage {
                role: "assistant".into(),
                content: prompt.clone(),
            },
            ChatMessage {
                role: "user".into(),
                content: "No puedo transferir el dominio porque el antirrobo sigue activo".into(),
            },
            ChatMessage {
                role: "assistant".into(),
                content: prompt,
            },
        ];
        let second = classify_turn(
            "mi correo es cliente@mi.com.co y la categoría es dominio",
            &history,
        );
        let Classification::Ask(confirm) = second else {
            panic!("debía pedir confirmación");
        };
        assert!(confirm.contains(CONFIRM_MARKER));
        assert!(confirm.contains("cliente@mi.com.co"));

        let mut full = history;
        full.push(ChatMessage {
            role: "user".into(),
            content: "mi correo es cliente@mi.com.co y la categoría es dominio".into(),
        });
        full.push(ChatMessage {
            role: "assistant".into(),
            content: confirm,
        });
        let third = classify_turn("CONFIRMO", &full);
        let Classification::Register(draft) = third else {
            panic!("debía registrar");
        };
        assert_eq!(draft.correo, "cliente@mi.com.co");
        assert_eq!(draft.categoria, "Dominio");
        assert!(draft.detalle.to_lowercase().contains("antirrobo"));
    }

    #[test]
    fn one_message_with_confirmation_registers() {
        let classification = classify_turn(
            "Quiero registrar un caso. Correo cliente@mi.com.co. Categoría dominio. No puedo transferir el dominio porque el antirrobo sigue activo. Confirmo.",
            &[],
        );
        let Classification::Register(draft) = classification else {
            panic!("debía registrar en un solo mensaje");
        };
        assert_eq!(draft.categoria, "Dominio");
        assert!(draft.detalle.to_lowercase().contains("transferir"));
    }

    #[test]
    fn ordinary_yes_is_not_a_case() {
        let classification = classify_turn("si se puede cambiar la contraseña", &[]);
        assert!(matches!(classification, Classification::Knowledge { .. }));
    }

    #[test]
    fn retrieval_query_keeps_recent_user_turns() {
        let history = vec![
            ChatMessage {
                role: "user".into(),
                content: "dominio vencido".into(),
            },
            ChatMessage {
                role: "assistant".into(),
                content: "te explico".into(),
            },
            ChatMessage {
                role: "user".into(),
                content: "dominio vencido".into(),
            },
        ];
        let query = build_retrieval_query("cuál es el plazo", &history);
        assert_eq!(query, "dominio vencido\ncuál es el plazo");
    }

    #[test]
    fn rejects_blank_questions() {
        assert!(validate_question("   ").is_err());
        assert!(validate_question(&"a".repeat(2001)).is_err());
        assert_eq!(validate_question("  hola  ").expect("pregunta"), "hola");
    }
}
