"""Limpieza y división inteligente de documentos de texto.

Estrategia de fragmentación:
1. Si el documento ya trae bloques `[CHUNK_ID]`, se respetan (típico de
   bases de conocimiento preparadas para RAG).
2. En caso contrario se divide por párrafos, luego por oraciones y
   finalmente por longitud máxima, evitando cortar palabras.
"""

from __future__ import annotations

import logging
import re
from typing import Any

from app.config.settings import settings
from app.models.schemas import TextChunk

logger = logging.getLogger(__name__)

STRUCTURED_SEPARATOR = re.compile(r"\n?={8,}\n?")
FIELD_PATTERN = re.compile(r"^\[([A-Z_]+)\]:\s*(.*)$")
SENTENCE_SPLIT = re.compile(r"(?<=[.!?])\s+(?=[A-ZÁÉÍÓÚÑ¿¡0-9])|(?<=[:;])\s+(?=[A-ZÁÉÍÓÚÑ¿¡])")
MULTI_BLANK = re.compile(r"[ \t]+")
MULTI_NEWLINES = re.compile(r"\n{3,}")


class DocumentProcessingError(Exception):
    """Error al leer o fragmentar un documento de texto."""

    def __init__(self, message: str, user_message: str | None = None) -> None:
        super().__init__(message)
        self.user_message = user_message or message


def clean_text(text: str) -> str:
    """Normaliza espacios sin eliminar el contenido relevante.

    Conserva saltos de párrafo (necesarios para chunking) y elimina
    espacios repetidos, tabulaciones y líneas en blanco excesivas.
    """
    if text is None:
        return ""

    normalized = text.replace("\r\n", "\n").replace("\r", "\n")
    normalized = normalized.replace("\ufeff", "")
    cleaned_lines = [MULTI_BLANK.sub(" ", line).rstrip() for line in normalized.split("\n")]
    cleaned = "\n".join(cleaned_lines)
    cleaned = MULTI_NEWLINES.sub("\n\n", cleaned)
    return cleaned.strip()


def parse_structured_document(text: str, source: str) -> list[TextChunk] | None:
    """Interpreta documentos con metadatos `[CHUNK_ID]`, `[TITLE]`, etc.

    Devuelve None si el archivo no usa ese formato, para que el llamador
    caiga en la estrategia genérica de párrafos/oraciones.
    """
    if "[CHUNK_ID]:" not in text:
        return None

    raw_blocks = [block.strip() for block in STRUCTURED_SEPARATOR.split(text) if block.strip()]
    parsed: list[TextChunk] = []
    sequential_id = 0

    for block in raw_blocks:
        fields: dict[str, str] = {}
        content_lines: list[str] = []
        in_content = False

        for line in block.split("\n"):
            if in_content:
                content_lines.append(line)
                continue

            match = FIELD_PATTERN.match(line.strip())
            if match:
                key, value = match.group(1), match.group(2).strip()
                if key == "CONTENT":
                    in_content = True
                    if value:
                        content_lines.append(value)
                else:
                    fields[key.lower()] = value
                continue

            if line.strip() and not set(line.strip()) <= {"-", "="}:
                content_lines.append(line)

        body = clean_text("\n".join(content_lines))
        title = fields.get("title", "").strip()
        category = fields.get("category", "").strip()
        summary = fields.get("summary", "").strip()
        doc_chunk_id = fields.get("chunk_id", "").strip()

        if not body and not title:
            continue

        header_parts = []
        if title:
            header_parts.append(title)
        if category:
            header_parts.append(f"Categoría: {category}")
        if summary:
            header_parts.append(summary)

        composed = "\n\n".join([*header_parts, body] if body else header_parts).strip()
        if not composed:
            continue

        pieces = _split_long_text(composed)
        total_pieces = len(pieces)
        for index, piece in enumerate(pieces):
            sequential_id += 1
            metadata: dict[str, Any] = {
                "source": source,
                "chunk_id": sequential_id,
            }
            if doc_chunk_id:
                metadata["doc_chunk_id"] = doc_chunk_id
                if total_pieces > 1:
                    metadata["doc_chunk_part"] = index + 1
            if title:
                metadata["title"] = title
            if category:
                metadata["category"] = category
            parsed.append(
                TextChunk(
                    content=piece,
                    chunk_id=sequential_id,
                    source=source,
                    metadata=metadata,
                )
            )

    if not parsed:
        return None

    logger.info("Documento estructurado detectado: %s bloques indexables.", len(parsed))
    return parsed


def split_into_chunks(
    text: str,
    source: str = "documento.txt",
    chunk_size_min: int | None = None,
    chunk_size_max: int | None = None,
    overlap: int | None = None,
) -> list[TextChunk]:
    """Divide texto limpio en fragmentos con solapamiento controlado."""
    chunk_size_min = chunk_size_min or settings.chunk_size_min
    chunk_size_max = chunk_size_max or settings.chunk_size_max
    overlap = overlap if overlap is not None else settings.chunk_overlap

    cleaned = clean_text(text)
    if not cleaned:
        raise DocumentProcessingError(
            "El documento está vacío después de la limpieza.",
            user_message="El archivo de conocimiento está vacío. Agrega contenido e inténtalo de nuevo.",
        )

    structured = parse_structured_document(cleaned, source)
    if structured:
        return structured

    units = _paragraphs_to_units(cleaned, chunk_size_max)
    packed = _pack_units(units, chunk_size_min, chunk_size_max, overlap)

    chunks: list[TextChunk] = []
    for index, content in enumerate(packed, start=1):
        chunks.append(
            TextChunk(
                content=content,
                chunk_id=index,
                source=source,
                metadata={"source": source, "chunk_id": index},
            )
        )

    logger.info("Documento dividido en %s fragmentos genéricos.", len(chunks))
    return chunks


def _split_long_text(text: str, max_size: int | None = None) -> list[str]:
    """Subdivide un bloque ya semántico si supera el tamaño máximo."""
    max_size = max_size or settings.chunk_size_max
    if len(text) <= max_size:
        return [text]

    units = _paragraphs_to_units(text, max_size)
    return _pack_units(units, settings.chunk_size_min, max_size, settings.chunk_overlap)


def _paragraphs_to_units(text: str, max_size: int) -> list[str]:
    """Convierte el texto en unidades que no superan max_size."""
    paragraphs = [p.strip() for p in re.split(r"\n\s*\n", text) if p.strip()]
    units: list[str] = []
    for paragraph in paragraphs or [text]:
        if len(paragraph) <= max_size:
            units.append(paragraph)
            continue
        for sentence in _split_sentences(paragraph):
            if len(sentence) <= max_size:
                units.append(sentence)
            else:
                units.extend(_split_by_length(sentence, max_size))
    return units


def _split_sentences(text: str) -> list[str]:
    parts = SENTENCE_SPLIT.split(text.strip())
    return [part.strip() for part in parts if part.strip()] or [text.strip()]


def _split_by_length(text: str, max_size: int) -> list[str]:
    """Corta por espacios para no partir palabras a la mitad."""
    pieces: list[str] = []
    remaining = text.strip()
    while remaining:
        if len(remaining) <= max_size:
            pieces.append(remaining)
            break
        window = remaining[: max_size + 1]
        cut = window.rfind(" ")
        if cut < max_size // 2:
            cut = max_size
        pieces.append(remaining[:cut].strip())
        remaining = remaining[cut:].strip()
    return [p for p in pieces if p]


def _pack_units(
    units: list[str],
    size_min: int,
    size_max: int,
    overlap: int,
) -> list[str]:
    """Agrupa unidades pequeñas y aplica solapamiento entre chunks."""
    chunks: list[str] = []
    buffer = ""

    def flush() -> None:
        nonlocal buffer
        if buffer.strip():
            chunks.append(buffer.strip())
            buffer = ""

    for unit in units:
        candidate = f"{buffer}\n\n{unit}".strip() if buffer else unit
        if len(candidate) <= size_max:
            buffer = candidate
            continue
        if buffer:
            flush()
            if chunks:
                buffer = _overlap_from(chunks[-1], overlap)
                buffer = f"{buffer}\n\n{unit}".strip() if buffer else unit
                if len(buffer) > size_max:
                    chunks.append(unit)
                    buffer = ""
            else:
                buffer = unit
        else:
            chunks.append(unit)

    if buffer:
        if chunks and len(buffer) < size_min and len(chunks[-1]) + len(buffer) <= size_max + overlap:
            chunks[-1] = f"{chunks[-1]}\n\n{buffer}".strip()
        else:
            chunks.append(buffer.strip())

    return chunks


def _overlap_from(previous: str, overlap: int) -> str:
    """Toma el final del chunk anterior respetando límites de palabra."""
    if overlap <= 0 or len(previous) <= overlap:
        return previous
    tail = previous[-overlap:]
    space = tail.find(" ")
    if space != -1 and space < len(tail) - 1:
        tail = tail[space + 1 :]
    return tail.strip()
