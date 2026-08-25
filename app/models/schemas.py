"""Esquemas de datos de la API y del pipeline de documentos."""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any, Optional

from pydantic import BaseModel, Field, field_validator


@dataclass
class TextChunk:
    """Fragmento de texto listo para indexar en ChromaDB."""

    content: str
    chunk_id: int
    source: str
    metadata: dict[str, Any] = field(default_factory=dict)


class ChatRequest(BaseModel):
    """Pregunta enviada por el chatbot."""

    question: str = Field(..., min_length=1, description="Pregunta del usuario")

    @field_validator("question")
    @classmethod
    def question_must_not_be_blank(cls, value: str) -> str:
        cleaned = value.strip()
        if not cleaned:
            raise ValueError("La pregunta no puede estar vacía.")
        if len(cleaned) > 2000:
            raise ValueError("La pregunta es demasiado larga (máximo 2000 caracteres).")
        return cleaned


class SourceChunk(BaseModel):
    """Referencia a un fragmento utilizado para construir la respuesta."""

    chunk_id: int
    source: str
    title: Optional[str] = None
    category: Optional[str] = None
    distance: Optional[float] = None


class ChatResponse(BaseModel):
    """Respuesta del asistente junto con las fuentes recuperadas."""

    answer: str
    sources: list[SourceChunk] = Field(default_factory=list)


class HealthResponse(BaseModel):
    """Estado del sistema expuesto en GET /health."""

    status: str
    vector_database: str
    indexed_chunks: int = 0
    claude_api: str = "not_configured"


class ReindexRequest(BaseModel):
    """Opciones para reprocesar el documento de conocimiento."""

    reset: bool = Field(
        default=True,
        description="Si es true, elimina la colección anterior antes de indexar.",
    )


class ReindexResponse(BaseModel):
    """Resultado del proceso de reindexación."""

    status: str
    message: str
    chunks_indexed: int = 0
    characters: int = 0
    source: str = ""
    reset: bool = False
