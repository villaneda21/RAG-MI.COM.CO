#!/usr/bin/env python
"""Indexa data/documents/documento.txt en ChromaDB.

Ejemplos:
    python scripts/ingest_document.py
    python scripts/ingest_document.py --reset
    python scripts/ingest_document.py --status
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
from app.services.vector_service import VectorService, VectorStoreError  # noqa: E402
from app.utils.text_processor import DocumentProcessingError  # noqa: E402

logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s | %(levelname)s | %(name)s | %(message)s",
)
logger = logging.getLogger("ingest")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Crea o actualiza la base vectorial empresa_knowledge_base."
    )
    parser.add_argument(
        "--reset",
        action="store_true",
        help="Elimina la colección anterior y la crea desde cero.",
    )
    parser.add_argument(
        "--force",
        action="store_true",
        help="Reindexa aunque el documento no haya cambiado (no borra otras fuentes).",
    )
    parser.add_argument(
        "--status",
        action="store_true",
        help="Muestra el estado de la colección y sale.",
    )
    parser.add_argument(
        "--document",
        type=str,
        default=str(settings.default_document_path),
        help="Ruta al archivo TXT a indexar.",
    )
    return parser.parse_args()


def show_status(vector_service: VectorService) -> int:
    try:
        vector_service.connect()
    except VectorStoreError as exc:
        logger.error("%s", exc)
        print(exc.user_message)
        return 1

    count = vector_service.count()
    meta = vector_service._read_meta()
    print("Colección:", vector_service.collection_name)
    print("Ruta:     ", vector_service.persist_directory)
    print("Chunks:   ", count)
    print("Modelo:   ", settings.embedding_model)
    if meta:
        print("Fuente:   ", meta.get("source"))
        print("Hash:     ", str(meta.get("sha256", ""))[:16] + "...")
        print("Fecha:    ", meta.get("indexed_at"))
    else:
        print("Aún no hay metadata de indexación.")
    return 0


def main() -> int:
    args = parse_args()
    document_path = Path(args.document)
    vector_service = VectorService()
    document_service = DocumentService(document_path=document_path)

    if args.status:
        return show_status(vector_service)

    logger.info("Inicio de ingestión de %s", document_path)
    if args.reset:
        logger.info("Modo reset: se eliminará la colección %s", settings.chroma_collection_name)

    try:
        chunks, characters, fingerprint = document_service.process()
        logger.info("Caracteres leídos: %s", characters)
        logger.info("Chunks a indexar: %s", len(chunks))

        reset = bool(args.reset)
        if args.force and not reset:
            # Cambia el hash almacenado para forzar el upsert.
            vector_service.connect()
            indexed = vector_service.index_chunks(chunks, fingerprint + "-force", reset=False)
        else:
            indexed = vector_service.index_chunks(chunks, fingerprint, reset=reset)
    except (DocumentProcessingError, VectorStoreError) as exc:
        logger.error("%s", exc)
        print(exc.user_message)
        return 1

    print(f"Indexación completada: {indexed} chunks ({characters} caracteres).")
    print(f"Colección: {settings.chroma_collection_name}")
    print(f"Persistencia: {settings.chroma_persist_directory}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
