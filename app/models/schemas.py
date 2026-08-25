"""Esquemas de datos de la API y del pipeline de documentos."""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any, Literal, Optional

from pydantic import BaseModel, Field, field_validator

HelpTypeId = Literal["correo", "dominio", "hosting", "facturacion"]
IntakeStep = Literal[
    "idle",
    "awaiting_email",
    "awaiting_reason",
    "helping",
    "awaiting_close",
    "closed",
]


@dataclass
class TextChunk:
    """Fragmento de texto listo para indexar en ChromaDB."""

    content: str
    chunk_id: int
    source: str
    metadata: dict[str, Any] = field(default_factory=dict)


class ChatTurn(BaseModel):
    """Turno previo de la conversación, para el flujo de atención."""

    role: Literal["user", "assistant"]
    content: str = Field(..., min_length=1, max_length=4000)


class IntakeState(BaseModel):
    """Estado del flujo de captura, orientación y cierre de caso."""

    active: bool = False
    step: IntakeStep = "idle"
    email: Optional[str] = None
    category: Optional[HelpTypeId] = None
    category_label: Optional[str] = None
    notes: Optional[str] = None
    summary: Optional[str] = None
    case_id: Optional[str] = None
    status: Optional[str] = None
    registered_at: Optional[str] = None
    registered_at_display: Optional[str] = None
    turn: int = 0
    needs_email: bool = False
    needs_reason: bool = False
    needs_close: bool = False
    show_categories: bool = False
    registered: bool = False
    blocked_connection: bool = False


class CaseRecord(BaseModel):
    """Registro persistido en el historial de la cuenta."""

    case_id: str
    email: str
    help_type: HelpTypeId | str
    help_type_label: str
    summary: str
    registered_at: str
    registered_at_display: str
    status: str = "Cerrado"


class ChatRequest(BaseModel):
    """Pregunta enviada por el chatbot."""

    question: str = Field(..., min_length=1, description="Pregunta del usuario")
    history: list[ChatTurn] = Field(default_factory=list)
    requested_category: Optional[HelpTypeId] = Field(
        default=None,
        description="Tipo de ayuda elegido: correo, dominio, hosting o facturación/compra.",
    )
    intake: Optional[IntakeState] = None

    @field_validator("question")
    @classmethod
    def question_must_not_be_blank(cls, value: str) -> str:
        cleaned = value.strip()
        if not cleaned:
            raise ValueError("La pregunta no puede estar vacía.")
        if len(cleaned) > 2000:
            raise ValueError("La pregunta es demasiado larga (máximo 2000 caracteres).")
        return cleaned

    @field_validator("history")
    @classmethod
    def history_must_stay_short(cls, value: list[ChatTurn]) -> list[ChatTurn]:
        return value[-12:]


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
    intake: Optional[IntakeState] = None
    case: Optional[CaseRecord] = None


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
