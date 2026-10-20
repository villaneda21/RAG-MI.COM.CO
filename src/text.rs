//! Limpieza y división de documentos. Porta `app/utils/text_processor.py`.

use std::sync::LazyLock;

use fancy_regex::Regex as FancyRegex;
use regex::Regex;
use serde_json::{Map, Value};

use crate::error::AppError;
use crate::models::TextChunk;

static MULTI_BLANK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[ \t]+").expect("regex"));
static MULTI_NEWLINES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\n{3,}").expect("regex"));
static STRUCTURED_SEPARATOR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\n?={8,}\n?").expect("regex"));
static FIELD_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\[([A-Z_]+)\]:\s*(.*)$").expect("regex"));
static PARAGRAPH_SPLIT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\n\s*\n").expect("regex"));
static SENTENCE_SPLIT: LazyLock<FancyRegex> = LazyLock::new(|| {
    FancyRegex::new(r"(?<=[.!?])\s+(?=[A-ZÁÉÍÓÚÑ¿¡0-9])|(?<=[:;])\s+(?=[A-ZÁÉÍÓÚÑ¿¡])")
        .expect("sentence regex")
});

#[derive(Debug, Clone, Copy)]
pub struct ChunkOptions {
    pub min: usize,
    pub max: usize,
    pub overlap: usize,
}

impl Default for ChunkOptions {
    fn default() -> Self {
        Self {
            min: 800,
            max: 1200,
            overlap: 200,
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
    options: ChunkOptions,
) -> Result<Vec<TextChunk>, AppError> {
    let cleaned = clean_text(text);
    if cleaned.is_empty() {
        return Err(AppError::document(
            "El documento está vacío después de la limpieza.",
            "El archivo de conocimiento está vacío. Agrega contenido e inténtalo de nuevo.",
        ));
    }
    if let Some(structured) = parse_structured_document(&cleaned, source, options) {
        return Ok(structured);
    }
    let units = paragraphs_to_units(&cleaned, options.max);
    let packed = pack_units(&units, options.min, options.max, options.overlap);
    let mut chunks = Vec::with_capacity(packed.len());
    for (index, content) in packed.into_iter().enumerate() {
        let chunk_id = (index + 1) as i64;
        let mut metadata = Map::new();
        metadata.insert("source".to_string(), Value::String(source.to_string()));
        metadata.insert("chunk_id".to_string(), Value::from(chunk_id));
        chunks.push(TextChunk {
            content,
            chunk_id,
            source: source.to_string(),
            metadata,
        });
    }
    tracing::info!(
        count = chunks.len(),
        "documento dividido en fragmentos genéricos"
    );
    Ok(chunks)
}

pub fn parse_structured_document(
    text: &str,
    source: &str,
    options: ChunkOptions,
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
        let mut fields: Map<String, Value> = Map::new();
        let mut content_lines: Vec<&str> = Vec::new();
        let mut in_content = false;

        for line in block.split('\n') {
            if in_content {
                content_lines.push(line);
                continue;
            }
            if let Some(caps) = FIELD_PATTERN.captures(line.trim()) {
                let key = caps.get(1).map(|m| m.as_str()).unwrap_or_default();
                let value = caps.get(2).map(|m| m.as_str().trim()).unwrap_or_default();
                if key == "CONTENT" {
                    in_content = true;
                    if !value.is_empty() {
                        content_lines.push(value);
                    }
                } else {
                    fields.insert(key.to_lowercase(), Value::String(value.to_string()));
                }
                continue;
            }
            let trimmed = line.trim();
            if !trimmed.is_empty() && !is_rule(trimmed) {
                content_lines.push(line);
            }
        }

        let body = clean_text(&content_lines.join("\n"));
        let title = field_text(&fields, "title");
        let category = field_text(&fields, "category");
        let summary = field_text(&fields, "summary");
        let doc_chunk_id = field_text(&fields, "chunk_id");
        if body.is_empty() && title.is_empty() {
            continue;
        }

        let mut header_parts = Vec::new();
        if !title.is_empty() {
            header_parts.push(title.clone());
        }
        if !category.is_empty() {
            header_parts.push(format!("Categoría: {category}"));
        }
        if !summary.is_empty() {
            header_parts.push(summary);
        }
        let composed = if body.is_empty() {
            header_parts.join("\n\n")
        } else {
            let mut parts = header_parts;
            parts.push(body);
            parts.join("\n\n")
        };
        let composed = composed.trim().to_string();
        if composed.is_empty() {
            continue;
        }

        let pieces = split_long_text(&composed, options);
        let total_pieces = pieces.len();
        for (index, piece) in pieces.into_iter().enumerate() {
            sequential_id += 1;
            let mut metadata = Map::new();
            metadata.insert("source".to_string(), Value::String(source.to_string()));
            metadata.insert("chunk_id".to_string(), Value::from(sequential_id));
            if !doc_chunk_id.is_empty() {
                metadata.insert(
                    "doc_chunk_id".to_string(),
                    Value::String(doc_chunk_id.clone()),
                );
                if total_pieces > 1 {
                    metadata.insert(
                        "doc_chunk_part".to_string(),
                        Value::from((index + 1) as i64),
                    );
                }
            }
            if !title.is_empty() {
                metadata.insert("title".to_string(), Value::String(title.clone()));
            }
            if !category.is_empty() {
                metadata.insert("category".to_string(), Value::String(category.clone()));
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
        None
    } else {
        tracing::info!(count = parsed.len(), "documento estructurado detectado");
        Some(parsed)
    }
}

fn field_text(fields: &Map<String, Value>, key: &str) -> String {
    fields
        .get(key)
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn is_rule(line: &str) -> bool {
    line.chars().all(|ch| ch == '-' || ch == '=')
}

fn split_long_text(text: &str, options: ChunkOptions) -> Vec<String> {
    if char_len(text) <= options.max {
        return vec![text.to_string()];
    }
    let units = paragraphs_to_units(text, options.max);
    pack_units(&units, options.min, options.max, options.overlap)
}

fn paragraphs_to_units(text: &str, max_size: usize) -> Vec<String> {
    let paragraphs: Vec<String> = PARAGRAPH_SPLIT
        .split(text)
        .map(|part| part.trim().to_string())
        .filter(|part| !part.is_empty())
        .collect();
    let paragraphs = if paragraphs.is_empty() {
        vec![text.to_string()]
    } else {
        paragraphs
    };
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
    let parts = match SENTENCE_SPLIT.split(trimmed).collect::<Result<Vec<_>, _>>() {
        Ok(parts) => parts
            .into_iter()
            .map(|part| part.trim().to_string())
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>(),
        Err(err) => {
            tracing::warn!(error = %err, "no se pudieron separar oraciones");
            Vec::new()
        }
    };
    if parts.is_empty() {
        vec![trimmed.to_string()]
    } else {
        parts
    }
}

fn split_by_length(text: &str, max_size: usize) -> Vec<String> {
    if max_size == 0 {
        return vec![text.trim().to_string()];
    }
    let mut pieces = Vec::new();
    let mut remaining: Vec<char> = text.trim().chars().collect();
    while !remaining.is_empty() {
        if remaining.len() <= max_size {
            let tail: String = remaining.iter().collect();
            if !tail.is_empty() {
                pieces.push(tail);
            }
            break;
        }
        let window_end = (max_size + 1).min(remaining.len());
        let mut cut = remaining[..window_end]
            .iter()
            .rposition(|ch| *ch == ' ')
            .unwrap_or(max_size);
        if cut < max_size / 2 {
            cut = max_size.min(remaining.len());
        }
        if cut == 0 {
            cut = 1;
        }
        let piece: String = remaining[..cut]
            .iter()
            .collect::<String>()
            .trim()
            .to_string();
        if !piece.is_empty() {
            pieces.push(piece);
        }
        remaining = remaining[cut..]
            .iter()
            .collect::<String>()
            .trim()
            .chars()
            .collect();
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
            format!("{buffer}\n\n{unit}")
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
                    format!("{overlap_text}\n\n{unit}")
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
        let trimmed = buffer.trim().to_string();
        if !trimmed.is_empty() {
            if let Some(last) = chunks.last_mut() {
                if char_len(&trimmed) < size_min
                    && char_len(last) + char_len(&trimmed) <= size_max + overlap
                {
                    *last = format!("{last}\n\n{trimmed}");
                } else {
                    chunks.push(trimmed);
                }
            } else {
                chunks.push(trimmed);
            }
        }
    }
    chunks
}

fn overlap_from(previous: &str, overlap: usize) -> String {
    let chars: Vec<char> = previous.chars().collect();
    if overlap == 0 || chars.len() <= overlap {
        return previous.to_string();
    }
    let tail: String = chars[chars.len() - overlap..].iter().collect();
    if let Some(space) = tail.find(' ') {
        if space + 1 < tail.len() {
            return tail[space + 1..].trim().to_string();
        }
    }
    tail.trim().to_string()
}

fn char_len(text: &str) -> usize {
    text.chars().count()
}

#[cfg(test)]
mod tests {
    use super::{clean_text, parse_structured_document, split_into_chunks, ChunkOptions};

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
    fn generate_chunks() {
        let text = format!(
            "{}\n\n{}\n\n{}",
            "Párrafo uno sobre dominios .co y registro en Colombia. ".repeat(20),
            "Párrafo dos sobre hosting, cPanel y certificados SSL. ".repeat(20),
            "Párrafo tres sobre correo corporativo y webmail. ".repeat(20)
        );
        let chunks = split_into_chunks(&text, "documento.txt", ChunkOptions::default()).unwrap();
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
    fn structured_chunks_keep_metadata() {
        let chunks =
            parse_structured_document(SAMPLE_STRUCTURED, "documento.txt", ChunkOptions::default())
                .expect("formato estructurado");
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
    fn clean_text_keeps_meaning() {
        let cleaned = clean_text("Hola   mundo\r\n\r\n\r\nSiguiente   párrafo.\t");
        assert!(cleaned.contains("Hola mundo"));
        assert!(cleaned.contains("Siguiente párrafo."));
        assert!(!cleaned.contains('\r'));
    }

    #[test]
    fn real_knowledge_base_chunks() {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("data/documents/documento.txt");
        let raw = std::fs::read_to_string(path).unwrap();
        let chunks = split_into_chunks(&raw, "documento.txt", ChunkOptions::default()).unwrap();
        assert!(chunks.len() > 5, "chunks={}", chunks.len());
        assert!(chunks
            .iter()
            .any(|chunk| chunk.metadata.contains_key("title")));
    }
}
