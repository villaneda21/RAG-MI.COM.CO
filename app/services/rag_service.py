"""Orquestación del flujo RAG: validar, recuperar, contextualizar y generar."""

from __future__ import annotations

import asyncio
import logging
from typing import Any

from app.config.settings import settings
from app.models.schemas import (
    CaseRecord,
    ChatResponse,
    ChatTurn,
    IntakeState,
    ReindexResponse,
    SourceChunk,
)
from app.services.case_store import CaseStore
from app.services.claude_service import ClaudeService, ClaudeServiceError
from app.services.document_service import DocumentService
from app.services.intake_service import (
    IntakeDecision,
    IntakeSnapshot,
    continue_intake,
    history_texts,
    is_connection_restricted,
    mark_ready_to_close,
    snapshot_from_payload,
    wrap_help_answer,
)
from app.services.vector_service import VectorService, VectorStoreError
from app.utils.text_processor import DocumentProcessingError

logger = logging.getLogger(__name__)

INSUFFICIENT_INFO_FALLBACK = (
    "No encontré información suficiente sobre esta pregunta en la base de conocimiento disponible."
)

CONNECTION_REFUSAL = (
    "Con gusto te ayudo con la base de conocimiento, pero desde aquí no hago cambios, "
    "ajustes ni verificaciones de conexiones: ni DNS, ni SPF/DKIM/DMARC, ni ping, "
    "ni el estado de red o servidores. Si quieres, abrimos un caso con tu correo "
    "y lo dejamos registrado para que quede en el historial de tu cuenta."
)


class RagService:
    """Punto de entrada del flujo de pregunta-respuesta aumentado con recuperación."""

    def __init__(
        self,
        document_service: DocumentService | None = None,
        vector_service: VectorService | None = None,
        claude_service: ClaudeService | None = None,
        case_store: CaseStore | None = None,
    ) -> None:
        self.document_service = document_service or DocumentService()
        self.vector_service = vector_service or VectorService()
        self.claude_service = claude_service or ClaudeService()
        self.case_store = case_store or CaseStore()

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

    def _intake_payload(self, snapshot: IntakeSnapshot, extras: dict[str, bool] | None = None) -> IntakeState:
        flags = extras or {}
        return IntakeState(
            active=snapshot.active,
            step=snapshot.step,
            email=snapshot.email,
            category=snapshot.category,
            category_label=snapshot.category_label,
            notes=snapshot.notes,
            summary=snapshot.summary,
            case_id=snapshot.case_id,
            status=snapshot.status,
            registered_at=snapshot.registered_at,
            registered_at_display=snapshot.registered_at_display,
            turn=snapshot.turn,
            needs_email=flags.get("needs_email", snapshot.step == "awaiting_email"),
            needs_reason=flags.get("needs_reason", snapshot.step == "awaiting_reason"),
            needs_close=flags.get("needs_close", snapshot.step == "awaiting_close"),
            show_categories=flags.get("show_categories", snapshot.step == "awaiting_reason"),
            registered=flags.get("registered", snapshot.step == "closed"),
            blocked_connection=flags.get("blocked_connection", snapshot.blocked_connection),
        )

    def _case_payload(self, record: dict[str, str] | None) -> CaseRecord | None:
        if not record:
            return None
        return CaseRecord(
            case_id=record["case_id"],
            email=record["email"],
            help_type=record["help_type"],
            help_type_label=record["help_type_label"],
            summary=record["summary"],
            registered_at=record["registered_at"],
            registered_at_display=record.get("registered_at_display") or record["registered_at"],
            status=record.get("status") or "Cerrado",
        )

    async def _answer_from_knowledge(self, question: str) -> tuple[str, list[SourceChunk]]:
        hits = await asyncio.to_thread(self.vector_service.query, question)
        logger.info("Chunks recuperados: %s.", len(hits))
        context = self.build_context(hits)
        sources = self.vector_service.hits_to_sources(hits)
        if not context.strip():
            return INSUFFICIENT_INFO_FALLBACK, []
        answer = await self.claude_service.generate_response(question, context)
        return answer, sources

    def _decision_response(
        self,
        decision: IntakeDecision,
        answer: str,
        sources: list[SourceChunk] | None = None,
        snapshot: IntakeSnapshot | None = None,
    ) -> ChatResponse:
        snap = snapshot or decision.snapshot
        return ChatResponse(
            answer=answer,
            sources=sources or [],
            intake=self._intake_payload(snap, decision.extras),
            case=self._case_payload(decision.case),
        )

    async def ask(
        self,
        question: str,
        history: list[ChatTurn] | None = None,
        requested_category: str | None = None,
        intake: IntakeState | None = None,
    ) -> ChatResponse:
        """Ejecuta el pipeline de atención y, cuando corresponde, el RAG."""
        cleaned = (question or "").strip()
        if not cleaned:
            raise ValueError("La pregunta no puede estar vacía.")

        logger.info("Consulta recibida (%s caracteres).", len(cleaned))
        user_history = history_texts(history)
        current = snapshot_from_payload(intake.model_dump() if intake else None)

        decision = continue_intake(
            cleaned,
            history=user_history,
            current=current,
            requested_category=requested_category,
            store=self.case_store,
        )

        if decision.handled and decision.skip_rag:
            logger.info(
                "Flujo de caso (step=%s, email=%s, category=%s).",
                decision.snapshot.step,
                decision.snapshot.email,
                decision.snapshot.category,
            )
            return self._decision_response(decision, decision.answer)

        if decision.handled and decision.wrap_after_rag:
            rag_question = decision.rag_question or cleaned
            try:
                kb_answer, sources = await self._answer_from_knowledge(rag_question)
            except VectorStoreError:
                raise
            except ClaudeServiceError:
                raise
            ready = mark_ready_to_close(decision.snapshot, kb_answer)
            wrapped = wrap_help_answer(kb_answer, ready)
            extras = {
                **decision.extras,
                "needs_close": True,
                "needs_email": False,
                "needs_reason": False,
                "show_categories": False,
            }
            wrapped_decision = IntakeDecision(
                handled=True,
                skip_rag=True,
                wrap_after_rag=False,
                answer=wrapped,
                snapshot=ready,
                extras=extras,
            )
            return self._decision_response(wrapped_decision, wrapped, sources, ready)

        if is_connection_restricted(cleaned):
            logger.info("Consulta de conexiones bloqueada (sin diagnóstico).")
            return ChatResponse(answer=CONNECTION_REFUSAL, sources=[])

        try:
            answer, sources = await self._answer_from_knowledge(cleaned)
        except VectorStoreError:
            raise
        except ClaudeServiceError:
            raise
        return ChatResponse(answer=answer, sources=sources)

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
