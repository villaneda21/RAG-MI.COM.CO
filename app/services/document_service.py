"""Lectura y preparación de los documentos de conocimiento de la empresa."""

from __future__ import annotations

import hashlib
import logging
from pathlib import Path

from app.config.settings import settings
from app.models.schemas import TextChunk
from app.utils.text_processor import DocumentProcessingError, clean_text, split_into_chunks

logger = logging.getLogger(__name__)

KNOWLEDGE_SUFFIXES = {".txt", ".md"}
SKIP_NAME_PREFIXES = (".", "_")
SKIP_FILENAMES = {"readme.md"}


def list_knowledge_files(directory: Path, fallback: Path | None = None) -> list[Path]:
    """Lista TXT/MD indexables. Los archivos que empiezan por _ no se indexan (plantillas)."""
    folder = Path(directory)
    files: list[Path] = []
    if folder.exists() and folder.is_dir():
        for path in sorted(folder.iterdir()):
            if not path.is_file():
                continue
            name = path.name
            lowered = name.lower()
            if name.startswith(SKIP_NAME_PREFIXES):
                continue
            if lowered in SKIP_FILENAMES:
                continue
            if path.suffix.lower() not in KNOWLEDGE_SUFFIXES:
                continue
            files.append(path)
    if files:
        return files
    if fallback and fallback.exists() and fallback.is_file():
        return [fallback]
    return []


class DocumentService:
    """Carga documentos de conocimiento y los convierte en chunks."""

    def __init__(
        self,
        document_path: Path | None = None,
        documents_directory: Path | None = None,
    ) -> None:
        if document_path is not None and documents_directory is None:
            self.document_path = Path(document_path)
            self.documents_directory = self.document_path.parent
        else:
            self.documents_directory = Path(documents_directory or settings.documents_directory)
            self.document_path = Path(document_path or settings.default_document_path)

    def list_knowledge_files(self) -> list[Path]:
        """TXT/MD de la carpeta de conocimiento, listos para indexar."""
        return list_knowledge_files(self.documents_directory, fallback=self.document_path)

    def load_raw_text(self) -> str:
        """Lee el archivo completo con UTF-8 (compatible con Windows)."""
        path = self.document_path
        logger.info("Carga del documento: %s", path)

        if not path.exists():
            raise DocumentProcessingError(
                f"No existe el archivo de conocimiento: {path}",
                user_message=(
                    "No se encontró el archivo de conocimiento. "
                    "Coloca un TXT o MD en data/documents/ y vuelve a indexar."
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

    def fingerprint(self, raw: str | None = None) -> str:
        text = raw if raw is not None else self.load_raw_text()
        return hashlib.sha256(text.encode("utf-8")).hexdigest()

    def chunk_text(self, raw: str, source: str | None = None) -> list[TextChunk]:
        cleaned = clean_text(raw)
        logger.info("Texto limpio: %s caracteres.", len(cleaned))
        origin = source or self.document_path.name
        chunks = split_into_chunks(cleaned, source=origin)
        if not chunks:
            raise DocumentProcessingError(
                "No se generaron fragmentos a partir del documento.",
                user_message="No fue posible dividir el documento en fragmentos útiles.",
            )
        logger.info("Chunks creados: %s (origen=%s).", len(chunks), origin)
        return chunks

    def process(self) -> tuple[list[TextChunk], int, str]:
        """Lee, limpia y fragmenta el documento.

        Returns:
            chunks: fragmentos con metadata.
            characters: tamaño original en caracteres.
            fingerprint: hash SHA-256 del contenido crudo (anti-duplicados).
        """
        raw = self.load_raw_text()
        chunks = self.chunk_text(raw, source=self.document_path.name)
        return chunks, len(raw), self.fingerprint(raw)
