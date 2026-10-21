//! Limpieza y división de documentos. Misma estrategia que `app/utils/text_processor.py`.

use std::sync::LazyLock;

use fancy_regex::Regex as FancyRegex;
use regex::Regex;
use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

use crate::models::TextChunk;

static STRUCTURED_SEPARATOR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\n?={8,}\n?").expect("separador estructurado"));
static FIELD_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\[([A-Z_]+)\]:\s*(.*)$").expect("campo estructurado"));
static SENTENCE_SPLIT: LazyLock<FancyRegex> = LazyLock::new(|| {
    FancyRegex::new(r"(?<=[.!?])\s+(?=[A-ZÁÉÍÓÚÑ¿¡0-9])|(?<=[:;])\s+(?=[A-ZÁÉÍÓÚÑ¿¡])")
        .expect("divisor de oraciones")
});
static MULTI_BLANK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[ \t]+").expect("espacios"));
static MULTI_NEWLINES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\n{3,}").expect("saltos"));
static PARAGRAPH_SPLIT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\n\s*\n").expect("párrafos"));

#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct DocumentProcessingError {
    pub message: String,
    pub user_message: String,
}

impl DocumentProcessingError {
    pub fn new(message: impl Into<String>, user_message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            user_message: user_message.into(),
        }
    }
}

pub fn clean_text(text: &str) -> String {
    let normalized = text
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .replace('\u{feff}', "");
    let cleaned_lines: Vec<String> = normalized
        .split('\n')
        .map(|line| MULTI_BLANK.replace_all(line, " ").trim_end().to_string())
        .collect();
    let cleaned = cleaned_lines.join("\n");
    MULTI_NEWLINES
        .replace_all(&cleaned, "\n\n")
        .trim()
        .to_string()
}

pub fn split_into_chunks(
    text: &str,
    source: &str,
    chunk_size_min: usize,
    chunk_size_max: usize,
    overlap: usize,
) -> Result<Vec<TextChunk>, DocumentProcessingError> {
    let cleaned = clean_text(text);
    if cleaned.is_empty() {
        return Err(DocumentProcessingError::new(
            "El documento está vacío después de la limpieza.",
            "El archivo de conocimiento está vacío. Agrega contenido e inténtalo de nuevo.",
        ));
    }
    if let Some(structured) =
        parse_structured_document(&cleaned, source, chunk_size_min, chunk_size_max, overlap)
    {
        return Ok(structured);
    }
    let units = paragraphs_to_units(&cleaned, chunk_size_max);
    let packed = pack_units(&units, chunk_size_min, chunk_size_max, overlap);
    let mut chunks = Vec::new();
    for (index, content) in packed.into_iter().enumerate() {
        let chunk_id = (index + 1) as i64;
        let mut metadata = Map::new();
        metadata.insert("source".into(), json!(source));
        metadata.insert("chunk_id".into(), json!(chunk_id));
        chunks.push(TextChunk {
            content,
            chunk_id,
            source: source.to_string(),
            metadata,
        });
    }
    tracing::info!(
        "Documento dividido en {} fragmentos genéricos.",
        chunks.len()
    );
    Ok(chunks)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuideBlock {
    pub chunk_id: String,
    pub title: String,
    pub keywords: String,
    pub body: String,
}

/// Bloques `[CHUNK_ID]` del documento de conocimiento, sin partir el cuerpo.
pub fn guide_blocks(text: &str) -> Vec<GuideBlock> {
    if !text.contains("[CHUNK_ID]:") {
        return Vec::new();
    }
    let mut blocks = Vec::new();
    for block in STRUCTURED_SEPARATOR
        .split(text)
        .map(str::trim)
        .filter(|block| !block.is_empty())
    {
        let mut fields: BTreeMap<String, String> = BTreeMap::new();
        let mut content_lines = Vec::new();
        let mut in_content = false;
        for line in block.split('\n') {
            if in_content {
                content_lines.push(line.to_string());
                continue;
            }
            let stripped = line.trim();
            if let Some(captures) = FIELD_PATTERN.captures(stripped) {
                let key = captures.get(1).map(|item| item.as_str()).unwrap_or("");
                let value = captures
                    .get(2)
                    .map(|item| item.as_str().trim())
                    .unwrap_or("");
                if key == "CONTENT" {
                    in_content = true;
                    if !value.is_empty() {
                        content_lines.push(value.to_string());
                    }
                } else {
                    fields.insert(key.to_lowercase(), value.to_string());
                }
                continue;
            }
        }
        let chunk_id = fields.get("chunk_id").cloned().unwrap_or_default();
        if chunk_id.trim().is_empty() {
            continue;
        }
        blocks.push(GuideBlock {
            chunk_id: chunk_id.trim().to_string(),
            title: fields.get("title").cloned().unwrap_or_default(),
            keywords: fields.get("keywords").cloned().unwrap_or_default(),
            body: clean_text(&content_lines.join("\n")),
        });
    }
    blocks
}

pub fn parse_structured_document(
    text: &str,
    source: &str,
    chunk_size_min: usize,
    chunk_size_max: usize,
    overlap: usize,
) -> Option<Vec<TextChunk>> {
    if !text.contains("[CHUNK_ID]:") {
        return None;
    }
    let raw_blocks: Vec<&str> = STRUCTURED_SEPARATOR
        .split(text)
        .map(str::trim)
        .filter(|block| !block.is_empty())
        .collect();
    let mut parsed = Vec::new();
    let mut sequential_id: i64 = 0;

    for block in raw_blocks {
        let mut fields: BTreeMap<String, String> = BTreeMap::new();
        let mut content_lines: Vec<String> = Vec::new();
        let mut in_content = false;
        for line in block.split('\n') {
            if in_content {
                content_lines.push(line.to_string());
                continue;
            }
            let stripped = line.trim();
            if let Some(captures) = FIELD_PATTERN.captures(stripped) {
                let key = captures.get(1).map(|m| m.as_str()).unwrap_or("");
                let value = captures.get(2).map(|m| m.as_str().trim()).unwrap_or("");
                if key == "CONTENT" {
                    in_content = true;
                    if !value.is_empty() {
                        content_lines.push(value.to_string());
                    }
                } else {
                    fields.insert(key.to_lowercase(), value.to_string());
                }
                continue;
            }
            if !stripped.is_empty() && !stripped.chars().all(|ch| ch == '-' || ch == '=') {
                content_lines.push(line.to_string());
            }
        }

        let body = clean_text(&content_lines.join("\n"));
        let title = fields.get("title").cloned().unwrap_or_default();
        let category = fields.get("category").cloned().unwrap_or_default();
        let summary = fields.get("summary").cloned().unwrap_or_default();
        let doc_chunk_id = fields.get("chunk_id").cloned().unwrap_or_default();
        if body.is_empty() && title.trim().is_empty() {
            continue;
        }
        let mut header_parts = Vec::new();
        if !title.trim().is_empty() {
            header_parts.push(title.trim().to_string());
        }
        if !category.trim().is_empty() {
            header_parts.push(format!("Categoría: {}", category.trim()));
        }
        if !summary.trim().is_empty() {
            header_parts.push(summary.trim().to_string());
        }
        let mut pieces_header = header_parts;
        if !body.is_empty() {
            pieces_header.push(body);
        }
        let composed = pieces_header.join("\n\n").trim().to_string();
        if composed.is_empty() {
            continue;
        }
        let pieces = split_long_text(&composed, chunk_size_min, chunk_size_max, overlap);
        let total_pieces = pieces.len();
        for (index, piece) in pieces.into_iter().enumerate() {
            sequential_id += 1;
            let mut metadata = Map::new();
            metadata.insert("source".into(), Value::String(source.to_string()));
            metadata.insert("chunk_id".into(), json!(sequential_id));
            if !doc_chunk_id.trim().is_empty() {
                metadata.insert(
                    "doc_chunk_id".into(),
                    Value::String(doc_chunk_id.trim().to_string()),
                );
                if total_pieces > 1 {
                    metadata.insert("doc_chunk_part".into(), json!(index as i64 + 1));
                }
            }
            if !title.trim().is_empty() {
                metadata.insert("title".into(), Value::String(title.trim().to_string()));
            }
            if !category.trim().is_empty() {
                metadata.insert(
                    "category".into(),
                    Value::String(category.trim().to_string()),
                );
            }
            parsed.push(TextChunk {
                content: piece,
                chunk_id: sequential_id,
                source: source.to_string(),
                metadata,
            });
        }
    }

    if parsed.is_empty() {
        return None;
    }
    tracing::info!(
        "Documento estructurado detectado: {} bloques indexables.",
        parsed.len()
    );
    Some(parsed)
}

fn split_long_text(text: &str, size_min: usize, max_size: usize, overlap: usize) -> Vec<String> {
    if char_len(text) <= max_size {
        return vec![text.to_string()];
    }
    let units = paragraphs_to_units(text, max_size);
    pack_units(&units, size_min, max_size, overlap)
}

fn paragraphs_to_units(text: &str, max_size: usize) -> Vec<String> {
    let mut paragraphs: Vec<String> = PARAGRAPH_SPLIT
        .split(text)
        .map(|part| part.trim().to_string())
        .filter(|part| !part.is_empty())
        .collect();
    if paragraphs.is_empty() {
        paragraphs.push(text.to_string());
    }
    let mut units = Vec::new();
    for paragraph in paragraphs {
        if char_len(&paragraph) <= max_size {
            units.push(paragraph);
            continue;
        }
        for sentence in split_sentences(&paragraph) {
            if char_len(&sentence) <= max_size {
                units.push(sentence);
            } else {
                units.extend(split_by_length(&sentence, max_size));
            }
        }
    }
    units
}

fn split_sentences(text: &str) -> Vec<String> {
    let trimmed = text.trim();
    let mut parts = Vec::new();
    let mut last = 0;
    for found in SENTENCE_SPLIT.find_iter(trimmed) {
        let Ok(mat) = found else {
            continue;
        };
        let part = trimmed[last..mat.start()].trim();
        if !part.is_empty() {
            parts.push(part.to_string());
        }
        last = mat.end();
    }
    let tail = trimmed[last..].trim();
    if !tail.is_empty() {
        parts.push(tail.to_string());
    }
    if parts.is_empty() && !trimmed.is_empty() {
        parts.push(trimmed.to_string());
    }
    parts
}

fn split_by_length(text: &str, max_size: usize) -> Vec<String> {
    let max_size = max_size.max(1);
    let mut pieces = Vec::new();
    let mut remaining = text.trim().to_string();
    while !remaining.is_empty() {
        let remaining_len = char_len(&remaining);
        if remaining_len <= max_size {
            pieces.push(remaining);
            break;
        }
        let window_len = (max_size + 1).min(remaining_len);
        let window = slice_chars(&remaining, 0, window_len);
        let mut cut = rfind_char(window, ' ').unwrap_or(max_size);
        if cut < max_size / 2 {
            cut = max_size;
        }
        if cut == 0 {
            cut = 1;
        }
        cut = cut.min(remaining_len);
        let piece = slice_chars(&remaining, 0, cut).trim().to_string();
        remaining = slice_chars(&remaining, cut, remaining_len)
            .trim()
            .to_string();
        if !piece.is_empty() {
            pieces.push(piece);
        }
    }
    pieces
}

fn pack_units(units: &[String], size_min: usize, size_max: usize, overlap: usize) -> Vec<String> {
    let mut chunks: Vec<String> = Vec::new();
    let mut buffer = String::new();

    for unit in units {
        let candidate = if buffer.is_empty() {
            unit.clone()
        } else {
            format!("{buffer}\n\n{unit}").trim().to_string()
        };
        if char_len(&candidate) <= size_max {
            buffer = candidate;
            continue;
        }
        if !buffer.is_empty() {
            let flushed = buffer.trim().to_string();
            if !flushed.is_empty() {
                chunks.push(flushed);
            }
            buffer.clear();
            if let Some(previous) = chunks.last() {
                let overlap_text = overlap_from(previous, overlap);
                buffer = if overlap_text.is_empty() {
                    unit.clone()
                } else {
                    format!("{overlap_text}\n\n{unit}").trim().to_string()
                };
                if char_len(&buffer) > size_max {
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

    if !buffer.is_empty() {
        let can_merge = chunks.last().is_some_and(|last| {
            char_len(&buffer) < size_min && char_len(last) + char_len(&buffer) <= size_max + overlap
        });
        if can_merge {
            let last = chunks.last_mut().expect("can_merge implica un chunk");
            *last = format!("{last}\n\n{buffer}").trim().to_string();
        } else {
            let trimmed = buffer.trim().to_string();
            if !trimmed.is_empty() {
                chunks.push(trimmed);
            }
        }
    }
    chunks
}

fn overlap_from(previous: &str, overlap: usize) -> String {
    let len = char_len(previous);
    if overlap == 0 || len <= overlap {
        return previous.to_string();
    }
    let tail = slice_chars(previous, len - overlap, len);
    if let Some(space) = find_char(tail, ' ') {
        if space < char_len(tail).saturating_sub(1) {
            return slice_chars(tail, space + 1, char_len(tail))
                .trim()
                .to_string();
        }
    }
    tail.trim().to_string()
}

fn char_len(text: &str) -> usize {
    text.chars().count()
}

fn byte_index(text: &str, char_index: usize) -> usize {
    text.char_indices()
        .nth(char_index)
        .map(|(index, _)| index)
        .unwrap_or(text.len())
}

fn slice_chars(text: &str, start: usize, end: usize) -> &str {
    let start = start.min(end);
    let a = byte_index(text, start);
    let b = byte_index(text, end);
    &text[a..b]
}

fn find_char(text: &str, needle: char) -> Option<usize> {
    text.chars().position(|ch| ch == needle)
}

fn rfind_char(text: &str, needle: char) -> Option<usize> {
    text.chars()
        .rev()
        .position(|ch| ch == needle)
        .map(|from_end| char_len(text) - from_end - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_STRUCTURED: &str = "
================================================================================
[CHUNK_ID]: GUA-001
[CATEGORY]: Guías Operativas
[TITLE]: Cómo crear tu cuenta
[SUMMARY]: Pasos para registrarte.
--------------------------------------------------------------------------------
[CONTENT]:
Accede a mi.com.co y haz clic en Crear Cuenta. Completa nombre, email y contraseña.
Verifica el correo electrónico para activar el acceso.
================================================================================

================================================================================
[CHUNK_ID]: GUA-002
[CATEGORY]: Guías Operativas
[TITLE]: Código de cliente
[SUMMARY]: Dónde encontrar el identificador CLI.
--------------------------------------------------------------------------------
[CONTENT]:
El Código de Cliente tiene el formato CLI-XXXXXX. Entra a Mi cuenta o Perfil
y ubica el campo CÓDIGO DE CLIENTE en la columna derecha.
================================================================================
";

    #[test]
    fn clean_text_keeps_meaning() {
        let messy = "Hola   mundo\r\n\r\n\r\nSiguiente   párrafo.\t";
        let cleaned = clean_text(messy);
        assert!(cleaned.contains("Hola mundo"));
        assert!(cleaned.contains("Siguiente párrafo."));
        assert!(!cleaned.contains('\r'));
    }

    #[test]
    fn generate_chunks() {
        let text = format!(
            "{}\n\n{}\n\n{}",
            "Párrafo uno sobre dominios .co y registro en Colombia. ".repeat(20),
            "Párrafo dos sobre hosting, cPanel y certificados SSL. ".repeat(20),
            "Párrafo tres sobre correo corporativo y webmail. ".repeat(20)
        );
        let chunks = split_into_chunks(&text, "documento.txt", 800, 1200, 200).unwrap();
        assert!(chunks.len() >= 2);
        assert!(chunks.iter().all(|chunk| !chunk.content.trim().is_empty()));
        assert!(chunks.iter().all(|chunk| {
            chunk.metadata.get("source").and_then(|v| v.as_str()) == Some("documento.txt")
        }));
        let ids: Vec<i64> = chunks.iter().map(|chunk| chunk.chunk_id).collect();
        let expected: Vec<i64> = (1..=chunks.len() as i64).collect();
        assert_eq!(ids, expected);
    }

    #[test]
    fn guide_blocks_keep_keywords() {
        let sample = "\
================================================================================
[CHUNK_ID]: GUA-007
[TITLE]: Cómo acceder a cPanel
[KEYWORDS]: hosting, cPanel, acceder a cPanel
[CONTENT]:
En Mis servicios abre Acceso y pulsa Acceder a cPanel.
================================================================================
";
        let blocks = guide_blocks(sample);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].chunk_id, "GUA-007");
        assert!(blocks[0].keywords.contains("cPanel"));
        assert!(blocks[0].body.contains("Acceder a cPanel"));
    }

    #[test]
    fn structured_chunks_keep_metadata() {
        let chunks = parse_structured_document(SAMPLE_STRUCTURED, "documento.txt", 800, 1200, 200)
            .expect("documento estructurado");
        assert_eq!(chunks.len(), 2);
        assert_eq!(
            chunks[0]
                .metadata
                .get("doc_chunk_id")
                .and_then(|v| v.as_str()),
            Some("GUA-001")
        );
        assert_eq!(
            chunks[0].metadata.get("title").and_then(|v| v.as_str()),
            Some("Cómo crear tu cuenta")
        );
        assert!(chunks[0].content.contains("Crear Cuenta"));
    }

    #[test]
    fn empty_text_raises() {
        let error = split_into_chunks("   \n", "vacio.txt", 800, 1200, 200).unwrap_err();
        assert!(!error.user_message.is_empty());
    }
}
