#!/usr/bin/env python
"""Indexa los archivos de data/documents/ en ChromaDB.

Uso típico:
    python scripts/ingest_document.py
    python scripts/ingest_document.py --status
    python scripts/ingest_document.py --file guia_renovacion.txt
    python scripts/ingest_document.py --reset
"""

from __future__ import annotations

import argparse
import logging
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

from app.config.settings import settings  # noqa: E402
from app.services.document_service import DocumentService  # noqa: E402
from app.services.rag_service import RagService  # noqa: E402
from app.services.vector_service import VectorService, VectorStoreError  # noqa: E402
from app.utils.text_processor import DocumentProcessingError  # noqa: E402

logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s | %(levelname)s | %(name)s | %(message)s",
)
logger = logging.getLogger("ingest")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Indexa la carpeta data/documents/ en la base vectorial."
    )
    parser.add_argument(
        "--reset",
        action="store_true",
        help="Elimina la colección y vuelve a indexar todos los archivos.",
    )
    parser.add_argument(
        "--force",
        action="store_true",
        help="Reindexa aunque el archivo no haya cambiado.",
    )
    parser.add_argument(
        "--status",
        action="store_true",
        help="Muestra el estado de cada archivo y sale.",
    )
    parser.add_argument(
        "--file",
        "--document",
        dest="document",
        type=str,
        default=None,
        help="Indexa solo este archivo (nombre o ruta).",
    )
    return parser.parse_args()


def resolve_file(raw: str, directory: Path) -> Path:
    candidate = Path(raw)
    if candidate.exists():
        return candidate
    nested = directory / raw
    if nested.exists():
        return nested
    raise SystemExit(f"No encontré el archivo: {raw}")


def show_status(rag: RagService) -> int:
    try:
        rag.vector_service.connect()
        status = rag.index_status()
    except VectorStoreError as exc:
        logger.error("%s", exc)
        print(exc.user_message)
        return 1

    print("Carpeta:  ", status.directory)
    print("Colección:", rag.vector_service.collection_name)
    print("Chunks:   ", status.indexed_chunks)
    print("Modelo:   ", settings.embedding_model)
    print("Pendiente:", "sí" if status.pending_changes else "no")
    print()
    if not status.files:
        print("No hay archivos .txt o .md para indexar.")
        return 0
    print(f"{'Archivo':<42} {'Estado':<10} {'Chunks':>6}  Actualizado")
    for item in status.files:
        label = {
            "indexed": "al día",
            "changed": "cambió",
            "new": "nuevo",
            "error": "error",
        }.get(item.status, item.status)
        stamp = item.indexed_at or "-"
        print(f"{item.name:<42} {label:<10} {item.chunks:>6}  {stamp}")
    return 0


def main() -> int:
    args = parse_args()
    vector_service = VectorService()
    document_service = DocumentService()
    rag = RagService(document_service=document_service, vector_service=vector_service)

    if args.status:
        return show_status(rag)

    if args.document:
        only = resolve_file(args.document, document_service.documents_directory)
        logger.info("Indexando solo %s", only)
        try:
            result = rag.reindex(reset=bool(args.reset), force=bool(args.force), only=only)
        except (DocumentProcessingError, VectorStoreError) as exc:
            logger.error("%s", exc)
            print(exc.user_message)
            return 1
        print(result.message)
        print(f"Fragmentos escritos: {result.chunks_indexed}")
        print(f"Colección: {settings.chroma_collection_name}")
        return 0

    logger.info("Inicio de ingestión en %s", document_service.documents_directory)
    try:
        result = rag.reindex(reset=bool(args.reset), force=bool(args.force))
    except (DocumentProcessingError, VectorStoreError) as exc:
        logger.error("%s", exc)
        print(exc.user_message)
        return 1

    print(result.message)
    print(f"Fragmentos escritos: {result.chunks_indexed}")
    print(f"Archivos indexados: {result.files_indexed} · omitidos: {result.files_skipped}")
    print(f"Colección: {settings.chroma_collection_name}")
    print(f"Persistencia: {settings.chroma_persist_directory}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
