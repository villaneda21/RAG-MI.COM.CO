"""Orquestación del flujo RAG: validar, recuperar, contextualizar y generar."""

from __future__ import annotations

import logging
from typing import Any

from app.config.settings import settings
from app.models.schemas import CaseSummary, ChatResponse, ReindexResponse, SourceChunk
from app.services.case_service import CaseService
from app.services.claude_service import ClaudeService, ClaudeServiceError, SupportTurn
from app.services.conversation_service import ConversationService
from app.services.document_service import DocumentService
from app.services.vector_service import VectorService, VectorStoreError
from app.utils.text_processor import DocumentProcessingError

logger = logging.getLogger(__name__)

INSUFFICIENT_INFO_FALLBACK = (
    "No encontré información suficiente sobre esta pregunta en la base de conocimiento disponible."
)


class RagService:
    """Punto de entrada del flujo de pregunta-respuesta aumentado con recuperación."""

    def __init__(
        self,
        document_service: DocumentService | None = None,
        vector_service: VectorService | None = None,
        claude_service: ClaudeService | None = None,
        case_service: CaseService | None = None,
        conversation_service: ConversationService | None = None,
    ) -> None:
        self.document_service = document_service or DocumentService()
        self.vector_service = vector_service or VectorService()
        self.claude_service = claude_service or ClaudeService()
        self.case_service = case_service or CaseService()
        self.conversation_service = conversation_service or ConversationService()

    def build_context(self, hits: list[dict[str, Any]]) -> str:
        """Concatena los fragmentos recuperados en un contexto delimitado."""
        if not hits:
            return ""

        sections: list[str] = []
        for index, hit in enumerate(hits, start=1):
            metadata = hit.get("metadata") or {}
            title = metadata.get("title") or f"Fragmento {metadata.get('chunk_id', index)}"
            chunk_id = metadata.get("chunk_id", index)
            source = metadata.get("source", settings.default_document_name)
            distance = hit.get("distance")
            score_line = f" | relevancia (distancia): {distance:.4f}" if isinstance(distance, float) else ""
            body = (hit.get("document") or "").strip()
            sections.append(
                f"[Chunk {index} | id={chunk_id} | fuente={source}{score_line} | {title}]\n{body}"
            )
        return "\n\n".join(sections)

    async def ask(
        self,
        question: str,
        history: list[dict[str, str]] | None = None,
        session_id: str | None = None,
    ) -> ChatResponse:
        """Atiende un turno: recupera guías y, si hace falta, registra el caso."""
        cleaned = (question or "").strip()
        if not cleaned:
            raise ValueError("La pregunta no puede estar vacía.")

        logger.info("Consulta de soporte recibida (%s caracteres).", len(cleaned))
        active_session, started_new = self.conversation_service.resolve_session(session_id)
        history_for_claude = [] if started_new else (history or [])

        hits, sources = self._retrieve(cleaned, history_for_claude)
        context = self.build_context(hits)

        try:
            turn: SupportTurn = await self.claude_service.generate_support_reply(
                cleaned,
                history=history_for_claude,
                session_id=active_session,
                case_service=self.case_service,
                context=context,
            )
        except ClaudeServiceError:
            raise

        case_summary = None
        if turn.case:
            case_summary = CaseSummary(
                case_id=turn.case.case_id,
                correo=turn.case.correo,
                categoria=turn.case.categoria,
                detalle=turn.case.detalle,
                registrado_en=turn.case.registrado_en,
                estado=turn.case.estado,
            )

        self.conversation_service.record_turn(
            session_id=active_session,
            question=cleaned,
            answer=turn.text,
            history=history_for_claude,
            case=turn.case,
        )
        return ChatResponse(
            answer=turn.text,
            sources=sources,
            session_id=active_session,
            case=case_summary,
            case_closed=case_summary is not None,
            new_chat=started_new,
        )

    def _retrieve(
        self,
        question: str,
        history: list[dict[str, str]] | None,
    ) -> tuple[list[dict[str, Any]], list[SourceChunk]]:
        """Busca en ChromaDB las guías y políticas más cercanas a la consulta."""
        query = build_retrieval_query(question, history)
        try:
            hits = self.vector_service.query(query, n_results=settings.retrieval_k)
        except VectorStoreError:
            logger.warning("No se pudo recuperar contexto de la base de conocimiento.")
            return [], []
        return hits, self.vector_service.hits_to_sources(hits)

    def reindex(self, reset: bool = True) -> ReindexResponse:
        """Vuelve a procesar el TXT y actualizar ChromaDB."""
        logger.info("Reindexación solicitada (reset=%s).", reset)
        try:
            chunks, characters, fingerprint = self.document_service.process()
            indexed = self.vector_service.index_chunks(chunks, fingerprint, reset=reset)
        except (DocumentProcessingError, VectorStoreError):
            raise

        message = (
            "Base de conocimiento recreada desde cero."
            if reset
            else "Documento procesado. Los chunks previos de la misma fuente fueron reemplazados si el contenido cambió."
        )
        return ReindexResponse(
            status="ok",
            message=message,
            chunks_indexed=indexed,
            characters=characters,
            source=self.document_service.document_path.name,
            reset=reset,
        )


def sources_as_dicts(sources: list[SourceChunk]) -> list[dict[str, Any]]:
    return [source.model_dump() for source in sources]


def build_retrieval_query(question: str, history: list[dict[str, str]] | None = None) -> str:
    """Arma la consulta semántica con el mensaje actual y turnos recientes del cliente."""
    parts: list[str] = []
    for item in (history or [])[-6:]:
        if (item.get("role") or "").strip() != "user":
            continue
        text = (item.get("content") or "").strip()
        if text:
            parts.append(text)
    cleaned = (question or "").strip()
    if cleaned:
        parts.append(cleaned)
    # Evita repetir el mismo texto si el historial ya lo trae.
    deduped: list[str] = []
    for part in parts:
        if not deduped or deduped[-1] != part:
            deduped.append(part)
    return "\n".join(deduped[-3:])
