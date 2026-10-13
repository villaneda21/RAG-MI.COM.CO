use std::collections::HashMap;

use regex::Regex;
use sha2::{Digest, Sha256};

#[derive(Debug, Clone)]
pub struct ChunkSettings {
    pub chunk_size_min: usize,
    pub chunk_size_max: usize,
    pub chunk_overlap: usize,
    pub structured_max: usize,
}

impl Default for ChunkSettings {
    fn default() -> Self {
        Self {
            chunk_size_min: 800,
            chunk_size_max: 1_200,
            chunk_overlap: 200,
            structured_max: 8_000,
        }
    }
}

#[derive(Debug, Clone)]
pub struct TextChunk {
    pub chunk_index: i32,
    pub source: String,
    pub doc_chunk_id: Option<String>,
    pub doc_chunk_part: Option<i32>,
    pub title: String,
    pub category: String,
    pub entity: String,
    pub keywords: Vec<String>,
    pub summary: String,
    pub content: String,
}

impl TextChunk {
    fn generic(chunk_index: i32, source: &str, content: String) -> Self {
        Self {
            chunk_index,
            source: source.to_string(),
            doc_chunk_id: None,
            doc_chunk_part: None,
            title: String::new(),
            category: String::new(),
            entity: String::new(),
            keywords: Vec::new(),
            summary: String::new(),
            content,
        }
    }
}

pub struct Patterns {
    structured_separator: Regex,
    field: Regex,
    multi_blank: Regex,
    multi_newlines: Regex,
}

impl Patterns {
    pub fn compile() -> Result<Self, regex::Error> {
        Ok(Self {
            structured_separator: Regex::new(r"\n?={8,}\n?")?,
            field: Regex::new(r"^\[([A-Z_]+)\]:\s*(.*)$")?,
            multi_blank: Regex::new(r"[ \t]+")?,
            multi_newlines: Regex::new(r"\n{3,}")?,
        })
    }
}

#[derive(Debug, thiserror::Error)]
#[error("{user_message}")]
pub struct ProcessError {
    user_message: String,
}

impl ProcessError {
    fn new(user_message: impl Into<String>) -> Self {
        Self {
            user_message: user_message.into(),
        }
    }
}

pub fn sha256_hex(text: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digest = Sha256::digest(text.as_bytes());
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

pub fn fold_search(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut previous_space = false;
    for ch in input.chars() {
        let mapped = fold_char(ch);
        if mapped == ' ' {
            if !previous_space && !out.is_empty() {
                out.push(' ');
                previous_space = true;
            }
        } else {
            out.push(mapped);
            previous_space = false;
        }
    }
    if out.ends_with(' ') {
        out.pop();
    }
    out
}

fn fold_char(ch: char) -> char {
    match ch {
        'á' | 'à' | 'ä' | 'â' | 'Á' | 'À' | 'Ä' | 'Â' => 'a',
        'é' | 'è' | 'ë' | 'ê' | 'É' | 'È' | 'Ë' | 'Ê' => 'e',
        'í' | 'ì' | 'ï' | 'î' | 'Í' | 'Ì' | 'Ï' | 'Î' => 'i',
        'ó' | 'ò' | 'ö' | 'ô' | 'Ó' | 'Ò' | 'Ö' | 'Ô' => 'o',
        'ú' | 'ù' | 'ü' | 'û' | 'Ú' | 'Ù' | 'Ü' | 'Û' => 'u',
        'ñ' | 'Ñ' => 'n',
        other if other.is_ascii_alphanumeric() => other.to_ascii_lowercase(),
        _ => ' ',
    }
}

pub fn cap_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

pub fn extract_chunk_id(folded: &str) -> String {
    for token in folded.split_whitespace() {
        if looks_like_chunk_id(token) {
            return token.to_uppercase();
        }
    }
    String::new()
}

pub fn single_like_token(folded: &str) -> String {
    let mut tokens = folded.split_whitespace();
    let Some(token) = tokens.next() else {
        return String::new();
    };
    if tokens.next().is_some() || token.len() < 4 || token.len() > 40 {
        return String::new();
    }
    token.to_string()
}

fn looks_like_chunk_id(token: &str) -> bool {
    let mut parts = token.split('-');
    let Some(prefix) = parts.next() else {
        return false;
    };
    let Some(number) = parts.next() else {
        return false;
    };
    parts.next().is_none()
        && (2..=5).contains(&prefix.len())
        && prefix.chars().all(|ch| ch.is_ascii_alphabetic())
        && number.len() == 3
        && number.chars().all(|ch| ch.is_ascii_digit())
}

pub fn clean_text(text: &str, patterns: &Patterns) -> String {
    let normalized = text
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .replace('\u{feff}', "");
    let cleaned_lines: Vec<String> = normalized
        .split('\n')
        .map(|line| {
            patterns
                .multi_blank
                .replace_all(line, " ")
                .trim_end()
                .to_string()
        })
        .collect();
    let joined = cleaned_lines.join("\n");
    patterns
        .multi_newlines
        .replace_all(&joined, "\n\n")
        .trim()
        .to_string()
}

pub fn split_into_chunks(
    raw: &str,
    source: &str,
    patterns: &Patterns,
    settings: &ChunkSettings,
) -> Result<Vec<TextChunk>, ProcessError> {
    let cleaned = clean_text(raw, patterns);
    if cleaned.is_empty() {
        return Err(ProcessError::new(
            "El archivo de conocimiento está vacío. Agrégale contenido antes de indexar.",
        ));
    }
    if cleaned.contains("[CHUNK_ID]:") {
        let chunks = parse_structured(&cleaned, source, patterns, settings);
        if chunks.is_empty() {
            return Err(ProcessError::new(
                "No fue posible dividir el documento en fragmentos útiles.",
            ));
        }
        return Ok(chunks);
    }

    let units = paragraphs_to_units(&cleaned, settings.chunk_size_max);
    let packed = pack_units(&units, settings);
    if packed.is_empty() {
        return Err(ProcessError::new(
            "No fue posible dividir el documento en fragmentos útiles.",
        ));
    }
    let mut chunks = Vec::new();
    for (index, content) in packed.into_iter().enumerate() {
        let chunk_index = i32::try_from(index + 1).unwrap_or(i32::MAX);
        chunks.push(TextChunk::generic(chunk_index, source, content));
    }
    Ok(chunks)
}

fn parse_structured(
    text: &str,
    source: &str,
    patterns: &Patterns,
    settings: &ChunkSettings,
) -> Vec<TextChunk> {
    let mut parsed = Vec::new();
    let mut sequential: i32 = 0;
    for block in patterns.structured_separator.split(text) {
        let block = block.trim();
        if block.is_empty() {
            continue;
        }
        let mut fields: HashMap<String, String> = HashMap::new();
        let mut content_lines: Vec<String> = Vec::new();
        let mut in_content = false;
        for line in block.lines() {
            if in_content {
                content_lines.push(line.to_string());
                continue;
            }
            if let Some(captures) = patterns.field.captures(line.trim()) {
                let key = captures
                    .get(1)
                    .map(|value| value.as_str().to_string())
                    .unwrap_or_default();
                let value = captures
                    .get(2)
                    .map(|item| item.as_str().trim().to_string())
                    .unwrap_or_default();
                if key == "CONTENT" {
                    in_content = true;
                    if !value.is_empty() {
                        content_lines.push(value);
                    }
                } else if !key.is_empty() {
                    fields.insert(key.to_lowercase(), value);
                }
                continue;
            }
            let trimmed = line.trim();
            if !trimmed.is_empty() && !trimmed.chars().all(|ch| ch == '-' || ch == '=') {
                content_lines.push(line.to_string());
            }
        }

        let Some(doc_chunk_id) = fields
            .get("chunk_id")
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        let title = fields.get("title").cloned().unwrap_or_default();
        let category = fields.get("category").cloned().unwrap_or_default();
        let entity = fields.get("entity").cloned().unwrap_or_default();
        let summary = fields.get("summary").cloned().unwrap_or_default();
        let keywords = split_keywords(fields.get("keywords").map(String::as_str).unwrap_or(""));
        let body = clean_text(&content_lines.join("\n"), patterns);
        let content = if !body.is_empty() {
            body
        } else if !summary.is_empty() {
            summary.clone()
        } else {
            title.clone()
        };
        if content.is_empty() {
            continue;
        }
        let pieces = if content.chars().count() > settings.structured_max {
            split_long_text(&content, settings)
        } else {
            vec![content]
        };
        let total = pieces.len();
        for (index, piece) in pieces.into_iter().enumerate() {
            sequential = sequential.saturating_add(1);
            let part = if total > 1 {
                Some(i32::try_from(index + 1).unwrap_or(1))
            } else {
                None
            };
            parsed.push(TextChunk {
                chunk_index: sequential,
                source: source.to_string(),
                doc_chunk_id: Some(doc_chunk_id.clone()),
                doc_chunk_part: part,
                title: title.clone(),
                category: category.clone(),
                entity: entity.clone(),
                keywords: keywords.clone(),
                summary: summary.clone(),
                content: piece,
            });
        }
    }
    parsed
}

fn split_keywords(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(str::to_string)
        .collect()
}

fn split_long_text(text: &str, settings: &ChunkSettings) -> Vec<String> {
    if text.chars().count() <= settings.chunk_size_max {
        return vec![text.to_string()];
    }
    let units = paragraphs_to_units(text, settings.chunk_size_max);
    let packed = pack_units(&units, settings);
    if packed.is_empty() {
        vec![text.to_string()]
    } else {
        packed
    }
}

fn paragraphs_to_units(text: &str, max_size: usize) -> Vec<String> {
    let paragraphs = split_paragraphs(text);
    let source = if paragraphs.is_empty() {
        vec![text.to_string()]
    } else {
        paragraphs
    };
    let mut units = Vec::new();
    for paragraph in source {
        if paragraph.chars().count() <= max_size {
            units.push(paragraph);
            continue;
        }
        for sentence in split_sentences(&paragraph) {
            if sentence.chars().count() <= max_size {
                units.push(sentence);
            } else {
                units.extend(split_by_length(&sentence, max_size));
            }
        }
    }
    units
}

fn split_paragraphs(text: &str) -> Vec<String> {
    let mut paragraphs = Vec::new();
    let mut current = String::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            if !current.trim().is_empty() {
                paragraphs.push(current.trim().to_string());
                current.clear();
            }
        } else {
            if !current.is_empty() {
                current.push('\n');
            }
            current.push_str(line);
        }
    }
    if !current.trim().is_empty() {
        paragraphs.push(current.trim().to_string());
    }
    paragraphs
}

fn split_sentences(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut parts = Vec::new();
    let mut start = 0;
    let mut index = 0;
    while index < chars.len() {
        if matches!(chars[index], '.' | '!' | '?' | ':' | ';') {
            let strong = matches!(chars[index], '.' | '!' | '?');
            let mut cursor = index + 1;
            let spaces_start = cursor;
            while cursor < chars.len() && chars[cursor].is_whitespace() {
                cursor += 1;
            }
            if cursor > spaces_start
                && cursor < chars.len()
                && sentence_start(chars[cursor], strong)
            {
                let piece: String = chars[start..=index].iter().collect::<String>();
                let piece = piece.trim().to_string();
                if !piece.is_empty() {
                    parts.push(piece);
                }
                start = cursor;
                index = cursor;
                continue;
            }
        }
        index += 1;
    }
    let tail: String = chars[start..].iter().collect::<String>();
    let tail = tail.trim().to_string();
    if !tail.is_empty() {
        parts.push(tail);
    }
    if parts.is_empty() {
        vec![text.trim().to_string()]
    } else {
        parts
    }
}

fn sentence_start(ch: char, strong: bool) -> bool {
    if strong {
        ch.is_uppercase() || ch.is_ascii_digit() || matches!(ch, '¿' | '¡')
    } else {
        ch.is_uppercase() || matches!(ch, '¿' | '¡')
    }
}

fn split_by_length(text: &str, max_size: usize) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut pieces = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        if chars.len() - start <= max_size {
            let piece: String = chars[start..].iter().collect::<String>();
            let piece = piece.trim().to_string();
            if !piece.is_empty() {
                pieces.push(piece);
            }
            break;
        }
        let window_end = (start + max_size + 1).min(chars.len());
        let window = &chars[start..window_end];
        let mut cut = window.iter().rposition(|ch| *ch == ' ').unwrap_or(max_size);
        if cut < max_size / 2 {
            cut = max_size.min(window.len());
        }
        if cut == 0 {
            cut = 1;
        }
        let piece: String = chars[start..start + cut].iter().collect::<String>();
        let piece = piece.trim().to_string();
        if !piece.is_empty() {
            pieces.push(piece);
        }
        start += cut;
        while start < chars.len() && chars[start] == ' ' {
            start += 1;
        }
    }
    pieces
}

fn pack_units(units: &[String], settings: &ChunkSettings) -> Vec<String> {
    let mut chunks: Vec<String> = Vec::new();
    let mut buffer = String::new();

    for unit in units {
        let candidate = if buffer.is_empty() {
            unit.clone()
        } else {
            format!("{buffer}\n\n{unit}")
        };
        if candidate.chars().count() <= settings.chunk_size_max {
            buffer = candidate;
            continue;
        }
        if !buffer.is_empty() {
            chunks.push(buffer.trim().to_string());
            buffer.clear();
            if let Some(previous) = chunks.last() {
                let overlap = overlap_from(previous, settings.chunk_overlap);
                buffer = if overlap.is_empty() {
                    unit.clone()
                } else {
                    format!("{overlap}\n\n{unit}")
                };
                if buffer.chars().count() > settings.chunk_size_max {
                    chunks.push(unit.clone());
                    buffer.clear();
                }
            } else {
                buffer = unit.clone();
            }
        } else {
            chunks.push(unit.clone());
        }
    }

    if !buffer.trim().is_empty() {
        let buffer_len = buffer.chars().count();
        let can_merge = chunks.last().is_some_and(|previous| {
            buffer_len < settings.chunk_size_min
                && previous.chars().count() + buffer_len
                    <= settings.chunk_size_max + settings.chunk_overlap
        });
        if can_merge {
            if let Some(previous) = chunks.last_mut() {
                *previous = format!("{previous}\n\n{}", buffer.trim());
            }
        } else {
            chunks.push(buffer.trim().to_string());
        }
    }
    chunks
        .into_iter()
        .filter(|chunk| !chunk.is_empty())
        .collect()
}

fn overlap_from(previous: &str, overlap: usize) -> String {
    let chars: Vec<char> = previous.chars().collect();
    if overlap == 0 || chars.len() <= overlap {
        return previous.trim().to_string();
    }
    let tail: String = chars[chars.len() - overlap..].iter().collect();
    let trimmed = match tail.find(' ') {
        Some(space) if space + 1 < tail.len() => tail[space + 1..].to_string(),
        _ => tail,
    };
    trimmed.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::project_root;
    use std::collections::BTreeSet;

    #[test]
    fn folds_spanish_accents_for_search() {
        assert_eq!(fold_search("Contraseña y Código"), "contrasena y codigo");
        assert_eq!(extract_chunk_id("ver gua-003 ahora"), "GUA-003");
        assert_eq!(single_like_token("cpanel"), "cpanel");
        assert_eq!(single_like_token("como cambiar"), "");
    }

    #[test]
    fn parses_the_official_knowledge_base() {
        let path = project_root().join("base_de_conocimiento_micomco_rag.txt");
        let raw = std::fs::read_to_string(&path).expect("base de conocimiento");
        let patterns = Patterns::compile().expect("patrones");
        let chunks = split_into_chunks(
            &raw,
            "base_de_conocimiento_micomco_rag.txt",
            &patterns,
            &ChunkSettings::default(),
        )
        .expect("fragmentos");
        let ids: BTreeSet<_> = chunks
            .iter()
            .filter_map(|chunk| chunk.doc_chunk_id.as_deref())
            .collect();
        for id in [
            "INTRO-001",
            "GUA-001",
            "GUA-015",
            "LEG-004",
            "DOM-004",
            "HST-005",
            "CFG-001",
            "DAT-003",
            "PRV-001",
            "ANX-002",
        ] {
            assert!(ids.contains(id), "falta {id}");
        }
        assert_eq!(
            ids.len(),
            36,
            "se esperaban 36 identificadores y hay {}",
            ids.len()
        );
        let password = chunks
            .iter()
            .find(|chunk| chunk.doc_chunk_id.as_deref() == Some("GUA-003"))
            .expect("GUA-003");
        assert!(password.content.contains("10 caracteres"));
        assert!(password
            .keywords
            .iter()
            .any(|keyword| keyword.contains("2FA")));
        assert!(chunks.iter().all(|chunk| !chunk.content.is_empty()));
    }

    #[test]
    fn generic_text_is_packed_under_the_maximum() {
        let paragraph = "Esta oracion de prueba cabe en una unidad. ".repeat(80);
        let patterns = Patterns::compile().expect("patrones");
        let settings = ChunkSettings {
            chunk_size_min: 400,
            chunk_size_max: 500,
            chunk_overlap: 40,
            structured_max: 8_000,
        };
        let chunks =
            split_into_chunks(&paragraph, "notas.txt", &patterns, &settings).expect("chunks");
        assert!(chunks.len() > 1);
        for chunk in &chunks {
            assert!(
                chunk.chars_len() <= 500,
                "fragmento de {} caracteres",
                chunk.chars_len()
            );
            assert!(chunk.doc_chunk_id.is_none());
        }
    }

    #[test]
    fn fingerprint_is_stable() {
        assert_eq!(sha256_hex("hola"), sha256_hex("hola"));
        assert_ne!(sha256_hex("hola"), sha256_hex("hola!"));
        assert_eq!(sha256_hex("hola").len(), 64);
    }

    impl TextChunk {
        fn chars_len(&self) -> usize {
            self.content.chars().count()
        }
    }
}
