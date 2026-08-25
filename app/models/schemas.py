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


class ChatMessage(BaseModel):
    """Turno previo de la conversación."""

    role: str = Field(..., description="user o assistant")
    content: str = Field(..., min_length=1)


class ChatRequest(BaseModel):
    """Pregunta enviada por el chatbot."""

    question: str = Field(..., min_length=1, description="Pregunta del usuario")
    session_id: Optional[str] = Field(default=None, description="Identificador de la conversación")
    history: list[ChatMessage] = Field(default_factory=list)

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


class CaseSummary(BaseModel):
    """Caso de soporte dejado en el historial."""

    case_id: str
    correo: str
    categoria: str
    detalle: str
    registrado_en: str
    estado: str


class ChatResponse(BaseModel):
    """Respuesta del asistente junto con las fuentes o el caso registrado."""

    answer: str
    sources: list[SourceChunk] = Field(default_factory=list)
    session_id: Optional[str] = None
    case: Optional[CaseSummary] = None
    case_closed: bool = False
    new_chat: bool = False


class ConversationMessage(BaseModel):
    """Turno persistido de un chat."""

    role: str
    content: str
    created_at: str = ""


class ConversationSummary(BaseModel):
    """Resumen de un chat para la pantalla de historial."""

    session_id: str
    created_at: str
    updated_at: str
    status: str
    case_id: str = ""
    correo: str = ""
    categoria: str = ""
    summary: str = ""
    message_count: int = 0


class ConversationDetail(ConversationSummary):
    """Chat completo, con todos los mensajes."""

    messages: list[ConversationMessage] = Field(default_factory=list)


class HealthResponse(BaseModel):
    """Estado del sistema expuesto en GET /health."""

    status: str
    vector_database: str
    indexed_chunks: int = 0
    claude_api: str = "not_configured"


class ReindexRequest(BaseModel):
    """Opciones para indexar la carpeta de conocimiento."""

    reset: bool = Field(
        default=False,
        description="Si es true, borra la colección y vuelve a indexar todos los archivos.",
    )
    force: bool = Field(
        default=False,
        description="Si es true, reindexa aunque el archivo no haya cambiado.",
    )


class ReindexResponse(BaseModel):
    """Resultado del proceso de indexación."""

    status: str
    message: str
    chunks_indexed: int = 0
    characters: int = 0
    source: str = ""
    sources: list[str] = Field(default_factory=list)
    files_indexed: int = 0
    files_skipped: int = 0
    reset: bool = False


class IndexFileStatus(BaseModel):
    """Estado de un archivo de la carpeta de conocimiento."""

    name: str
    status: str
    chunks: int = 0
    characters: int = 0
    indexed_at: str = ""


class IndexStatusResponse(BaseModel):
    """Resumen de la base vectorial y de los archivos pendientes."""

    directory: str
    indexed_chunks: int = 0
    pending_changes: bool = False
    files: list[IndexFileStatus] = Field(default_factory=list)
