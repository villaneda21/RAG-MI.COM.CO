"""Orquestación del flujo RAG: validar, recuperar, contextualizar y generar."""

from __future__ import annotations

import logging
from typing import Any

from app.config.settings import settings
from app.models.schemas import ChatResponse, ReindexResponse, SourceChunk
from app.services.claude_service import ClaudeService, ClaudeServiceError
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
    ) -> None:
        self.document_service = document_service or DocumentService()
        self.vector_service = vector_service or VectorService()
        self.claude_service = claude_service or ClaudeService()

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

    async def ask(self, question: str) -> ChatResponse:
        """Ejecuta el pipeline completo para una pregunta del usuario."""
        cleaned = (question or "").strip()
        if not cleaned:
            raise ValueError("La pregunta no puede estar vacía.")

        logger.info("Consulta recibida (%s caracteres).", len(cleaned))

        try:
            hits = self.vector_service.query(cleaned)
        except VectorStoreError:
            raise

        logger.info("Chunks recuperados: %s.", len(hits))
        context = self.build_context(hits)
        sources = self.vector_service.hits_to_sources(hits)

        if not context.strip():
            return ChatResponse(answer=INSUFFICIENT_INFO_FALLBACK, sources=[])

        try:
            answer = await self.claude_service.generate_response(cleaned, context)
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
