"""Persistencia y búsqueda semántica sobre ChromaDB."""

from __future__ import annotations

import json
import logging
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Callable, Optional

import chromadb
from chromadb.api.types import EmbeddingFunction

from app.config.settings import settings
from app.models.schemas import SourceChunk, TextChunk

logger = logging.getLogger(__name__)

EmbeddingFactory = Callable[[], EmbeddingFunction]


class VectorStoreError(Exception):
    """Error al interactuar con ChromaDB o los embeddings."""

    def __init__(self, message: str, user_message: str | None = None) -> None:
        super().__init__(message)
        self.user_message = user_message or (
            "No se pudo consultar la base de conocimiento. Inténtalo de nuevo en unos minutos."
        )


def _default_embedding_factory() -> EmbeddingFunction:
    """Crea la función de embeddings gestionada por ChromaDB."""
    try:
        from chromadb.utils.embedding_functions import SentenceTransformerEmbeddingFunction
    except ImportError as exc:  # pragma: no cover - dependencia declarada
        raise VectorStoreError(
            f"No se pudo importar SentenceTransformerEmbeddingFunction: {exc}",
            user_message="Falta el componente de embeddings. Revisa la instalación de dependencias.",
        ) from exc

    model_name = settings.embedding_model
    logger.info("Inicializando modelo de embeddings: %s", model_name)
    try:
        return SentenceTransformerEmbeddingFunction(model_name=model_name)
    except Exception as exc:  # noqa: BLE001
        logger.exception("Error al generar / cargar embeddings.")
        raise VectorStoreError(
            f"Error al inicializar embeddings ({model_name}): {exc}",
            user_message="No se pudieron generar los embeddings del documento. Revisa el modelo configurado.",
        ) from exc


class VectorService:
    """Cliente persistente de ChromaDB para la colección de la empresa."""

    def __init__(
        self,
        persist_directory: Path | None = None,
        collection_name: str | None = None,
        embedding_factory: EmbeddingFactory | None = None,
        meta_path: Path | None = None,
    ) -> None:
        self.persist_directory = Path(persist_directory or settings.chroma_persist_directory)
        self.collection_name = collection_name or settings.chroma_collection_name
        self.meta_path = Path(meta_path or settings.index_meta_path)
        self._embedding_factory = embedding_factory or _default_embedding_factory
        self._client: Optional[Any] = None
        self._embedding_fn: Optional[EmbeddingFunction] = None

    def connect(self) -> Any:
        """Abre (o crea) el almacén persistente en chroma_db/."""
        self.persist_directory.mkdir(parents=True, exist_ok=True)
        try:
            self._client = chromadb.PersistentClient(path=str(self.persist_directory))
            self._client.heartbeat()
            logger.info("ChromaDB conectado en %s", self.persist_directory)
            return self._client
        except Exception as exc:  # noqa: BLE001
            logger.exception("Error al conectar con ChromaDB.")
            raise VectorStoreError(
                f"Error al conectar con ChromaDB: {exc}",
                user_message="No se pudo conectar con la base vectorial. Verifica que chroma_db/ sea accesible.",
            ) from exc

    @property
    def client(self) -> Any:
        if self._client is None:
            return self.connect()
        return self._client

    def is_connected(self) -> bool:
        try:
            self.client.heartbeat()
            return True
        except Exception:  # noqa: BLE001
            logger.exception("Heartbeat de ChromaDB falló.")
            return False

    def _embedding_function(self) -> EmbeddingFunction:
        if self._embedding_fn is None:
            self._embedding_fn = self._embedding_factory()
        return self._embedding_fn

    def get_collection(self, create: bool = True) -> Any:
        """Obtiene la colección, creándola si se solicita."""
        try:
            if create:
                return self.client.get_or_create_collection(
                    name=self.collection_name,
                    embedding_function=self._embedding_function(),
                    metadata={"hnsw:space": "cosine"},
                )
            return self.client.get_collection(
                name=self.collection_name,
                embedding_function=self._embedding_function(),
            )
        except Exception as exc:  # noqa: BLE001
            logger.exception("Error al obtener la colección %s.", self.collection_name)
            raise VectorStoreError(
                f"Error al acceder a la colección '{self.collection_name}': {exc}",
                user_message="La base de conocimiento no está disponible. Ejecuta la indexación e inténtalo de nuevo.",
            ) from exc

    def count(self) -> int:
        try:
            collection = self.client.get_collection(name=self.collection_name)
            return int(collection.count())
        except Exception:  # noqa: BLE001
            return 0

    def reset_collection(self) -> None:
        """Elimina por completo la colección anterior y su metadata de índice."""
        logger.info("Eliminando colección anterior: %s", self.collection_name)
        try:
            self.client.delete_collection(self.collection_name)
        except Exception:  # noqa: BLE001
            logger.info("La colección %s no existía o ya había sido eliminada.", self.collection_name)
        if self.meta_path.exists():
            self.meta_path.unlink()
        logger.info("Colección eliminada. Lista para recrearse desde cero.")

    def index_chunks(
        self,
        chunks: list[TextChunk],
        fingerprint: str,
        reset: bool = False,
    ) -> int:
        """Indexa fragmentos evitando duplicados entre ejecuciones.

        - Si `reset` es True, borra la colección y la vuelve a crear.
        - Si el hash del documento no cambió, no reindexa.
        - Si el documento cambió, reemplaza los chunks de esa fuente.
        """
        if not chunks:
            raise VectorStoreError(
                "No hay chunks para indexar.",
                user_message="No hay fragmentos para guardar en la base de conocimiento.",
            )

        logger.info("Inicio de la indexación (%s chunks, reset=%s).", len(chunks), reset)

        if reset:
            self.reset_collection()
        elif self._same_fingerprint(fingerprint) and self.count() > 0:
            logger.info("El documento no cambió (hash=%s). Se omite la reindexación.", fingerprint[:12])
            return self.count()

        collection = self.get_collection(create=True)
        source = chunks[0].source
        self._delete_source(collection, source)

        ids: list[str] = []
        documents: list[str] = []
        metadatas: list[dict[str, Any]] = []

        for chunk in chunks:
            ids.append(f"{chunk.source}::{chunk.chunk_id}")
            documents.append(chunk.content)
            metadatas.append(_chroma_safe_metadata(chunk.metadata))

        try:
            collection.upsert(ids=ids, documents=documents, metadatas=metadatas)
        except Exception as exc:  # noqa: BLE001
            logger.exception("Error al almacenar embeddings en ChromaDB.")
            raise VectorStoreError(
                f"Error al indexar en ChromaDB: {exc}",
                user_message="No se pudieron guardar los fragmentos en la base vectorial.",
            ) from exc

        self._write_meta(fingerprint, len(chunks), source)
        logger.info("Indexación finalizada: %s chunks en '%s'.", len(chunks), self.collection_name)
        return len(chunks)

    def query(self, question: str, n_results: int | None = None) -> list[dict[str, Any]]:
        """Busca los fragmentos más relevantes y sus distancias."""
        n_results = n_results or settings.retrieval_k
        if self.count() == 0:
            raise VectorStoreError(
                "La colección está vacía.",
                user_message=(
                    "La base de conocimiento aún no tiene información indexada. "
                    "Ejecuta python scripts/ingest_document.py o usa Reindexar."
                ),
            )

        try:
            collection = self.get_collection(create=False)
            result = collection.query(
                query_texts=[question],
                n_results=min(n_results, max(self.count(), 1)),
                include=["documents", "metadatas", "distances"],
            )
        except VectorStoreError:
            raise
        except Exception as exc:  # noqa: BLE001
            logger.exception("Error al consultar ChromaDB.")
            raise VectorStoreError(
                f"Error al consultar ChromaDB: {exc}",
                user_message="No se pudo buscar en la base de conocimiento. Inténtalo de nuevo.",
            ) from exc

        documents = (result.get("documents") or [[]])[0]
        metadatas = (result.get("metadatas") or [[]])[0]
        distances = (result.get("distances") or [[]])[0]
        ids = (result.get("ids") or [[]])[0]

        hits: list[dict[str, Any]] = []
        for index, document in enumerate(documents):
            metadata = metadatas[index] if index < len(metadatas) else {}
            distance = distances[index] if index < len(distances) else None
            hits.append(
                {
                    "id": ids[index] if index < len(ids) else None,
                    "document": document,
                    "metadata": metadata or {},
                    "distance": float(distance) if distance is not None else None,
                }
            )

        logger.info("Consulta recuperó %s chunks.", len(hits))
        return hits

    def hits_to_sources(self, hits: list[dict[str, Any]]) -> list[SourceChunk]:
        sources: list[SourceChunk] = []
        seen: set[tuple[Any, Any]] = set()
        for hit in hits:
            metadata = hit.get("metadata") or {}
            chunk_id = int(metadata.get("chunk_id") or 0)
            source = str(metadata.get("source") or settings.default_document_name)
            key = (source, chunk_id)
            if key in seen:
                continue
            seen.add(key)
            sources.append(
                SourceChunk(
                    chunk_id=chunk_id,
                    source=source,
                    title=metadata.get("title"),
                    category=metadata.get("category"),
                    distance=hit.get("distance"),
                )
            )
        return sources

    def _delete_source(self, collection: Any, source: str) -> None:
        """Elimina chunks previos de la misma fuente para evitar duplicados."""
        try:
            existing = collection.get(where={"source": source}, include=[])
            ids = existing.get("ids") or []
            if ids:
                collection.delete(ids=ids)
                logger.info("Se eliminaron %s chunks previos de '%s'.", len(ids), source)
        except Exception:  # noqa: BLE001
            logger.info("No había chunks previos que eliminar para '%s'.", source)

    def _same_fingerprint(self, fingerprint: str) -> bool:
        meta = self._read_meta()
        return bool(meta) and meta.get("sha256") == fingerprint

    def _read_meta(self) -> dict[str, Any]:
        if not self.meta_path.exists():
            return {}
        try:
            return json.loads(self.meta_path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError):
            return {}

    def _write_meta(self, fingerprint: str, chunks: int, source: str) -> None:
        payload = {
            "sha256": fingerprint,
            "chunks": chunks,
            "source": source,
            "collection": self.collection_name,
            "embedding_model": settings.embedding_model,
            "indexed_at": datetime.now(timezone.utc).isoformat(),
        }
        self.meta_path.parent.mkdir(parents=True, exist_ok=True)
        self.meta_path.write_text(json.dumps(payload, indent=2), encoding="utf-8")


def _chroma_safe_metadata(metadata: dict[str, Any]) -> dict[str, Any]:
    """Chroma solo acepta str, int, float o bool en metadata."""
    safe: dict[str, Any] = {}
    for key, value in metadata.items():
        if value is None:
            continue
        if isinstance(value, (str, int, float, bool)):
            safe[key] = value
        else:
            safe[key] = str(value)
    return safe
