"""Lectura y preparación del archivo TXT de la empresa."""

from __future__ import annotations

import hashlib
import logging
from pathlib import Path

from app.config.settings import settings
from app.models.schemas import TextChunk
from app.utils.text_processor import DocumentProcessingError, clean_text, split_into_chunks

logger = logging.getLogger(__name__)


class DocumentService:
    """Carga el documento de conocimiento y lo convierte en chunks."""

    def __init__(self, document_path: Path | None = None) -> None:
        self.document_path = Path(document_path or settings.default_document_path)

    def load_raw_text(self) -> str:
        """Lee el TXT completo con UTF-8 (compatible con Windows)."""
        path = self.document_path
        logger.info("Carga del documento: %s", path)

        if not path.exists():
            raise DocumentProcessingError(
                f"No existe el archivo de conocimiento: {path}",
                user_message=(
                    "No se encontró el archivo de conocimiento. "
                    "Coloca el TXT en data/documents/documento.txt y vuelve a intentar."
                ),
            )

        try:
            raw = path.read_text(encoding="utf-8")
        except UnicodeDecodeError:
            logger.warning("UTF-8 falló para %s; se reintenta con latin-1.", path)
            raw = path.read_text(encoding="latin-1")
        except OSError as exc:
            logger.exception("Error de E/S al leer el documento.")
            raise DocumentProcessingError(
                f"No se pudo leer el documento: {exc}",
                user_message="No se pudo leer el archivo de conocimiento. Verifica permisos y formato.",
            ) from exc

        if not raw.strip():
            raise DocumentProcessingError(
                f"El archivo está vacío: {path}",
                user_message="El archivo de conocimiento está vacío. Agrégale contenido antes de indexar.",
            )

        logger.info("Documento leído: %s caracteres.", len(raw))
        return raw

    def process(self) -> tuple[list[TextChunk], int, str]:
        """Lee, limpia y fragmenta el documento.

        Returns:
            chunks: fragmentos con metadata.
            characters: tamaño original en caracteres.
            fingerprint: hash SHA-256 del contenido crudo (anti-duplicados).
        """
        raw = self.load_raw_text()
        characters = len(raw)
        fingerprint = hashlib.sha256(raw.encode("utf-8")).hexdigest()
        cleaned = clean_text(raw)
        logger.info("Texto limpio: %s caracteres.", len(cleaned))

        chunks = split_into_chunks(cleaned, source=self.document_path.name)
        if not chunks:
            raise DocumentProcessingError(
                "No se generaron fragmentos a partir del documento.",
                user_message="No fue posible dividir el documento en fragmentos útiles.",
            )

        logger.info("Chunks creados: %s (origen=%s).", len(chunks), self.document_path.name)
        return chunks, characters, fingerprint
