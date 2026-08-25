"""Configuración centralizada del sistema RAG.

Todas las rutas, tamaños de chunk, modelos y claves se definen aquí
para que el resto del código no hardcodee valores de entorno.
"""

from __future__ import annotations

import logging
import os
from pathlib import Path

from dotenv import load_dotenv


def _find_project_root() -> Path:
    """Localiza la raíz del proyecto de forma independiente del cwd."""
    current = Path(__file__).resolve()
    for parent in current.parents:
        if (parent / "run.py").exists() and (parent / "requirements.txt").exists():
            return parent
    return current.parents[2]


PROJECT_ROOT = _find_project_root()
load_dotenv(PROJECT_ROOT / ".env")

# Evita llamadas de telemetría de ChromaDB al iniciar.
os.environ.setdefault("ANONYMIZED_TELEMETRY", "False")
os.environ.setdefault("CHROMA_TELEMETRY_IMPL", "none")
logging.getLogger("chromadb.telemetry.product.posthog").setLevel(logging.CRITICAL)

_API_KEY_PLACEHOLDERS = {
    "tu_api_key_aqui",
    "your_api_key_here",
    "changeme",
    "api_key",
    "sk-ant-xxxxxxxx",
}


def is_usable_anthropic_key(key: str | None) -> bool:
    """True si hay una clave real, no el placeholder de .env.example."""
    cleaned = (key or "").strip()
    if not cleaned:
        return False
    lowered = cleaned.lower()
    if lowered in _API_KEY_PLACEHOLDERS:
        return False
    if "aqui" in lowered or "your_api" in lowered or "replace" in lowered:
        return False
    return True


class Settings:
    """Parámetros de ejecución del backend, la base vectorial y Claude."""

    project_root: Path = PROJECT_ROOT

    # --- API Anthropic ---
    anthropic_api_key: str = os.getenv("ANTHROPIC_API_KEY", "").strip()
    claude_model: str = os.getenv("CLAUDE_MODEL", "claude-sonnet-5").strip()
    claude_max_tokens: int = int(os.getenv("CLAUDE_MAX_TOKENS", "1200"))
    claude_temperature: float = float(os.getenv("CLAUDE_TEMPERATURE", "0.2"))
    # Si el modelo configurado fue retirado (p. ej. Sonnet 4 en junio 2026), se prueban estos.
    claude_fallback_models: tuple[str, ...] = (
        "claude-sonnet-5",
        "claude-sonnet-4-6",
        "claude-sonnet-4-5-20250929",
    )

    # --- Embeddings ---
    embedding_model: str = os.getenv("EMBEDDING_MODEL", "all-MiniLM-L6-v2").strip()

    # --- ChromaDB ---
    chroma_persist_directory: Path = PROJECT_ROOT / "chroma_db"
    chroma_collection_name: str = os.getenv(
        "CHROMA_COLLECTION_NAME", "empresa_knowledge_base"
    ).strip()
    index_meta_path: Path = PROJECT_ROOT / "chroma_db" / ".index_meta.json"

    # --- Documento de conocimiento ---
    documents_directory: Path = PROJECT_ROOT / "data" / "documents"
    default_document_name: str = "documento.txt"
    default_document_path: Path = documents_directory / default_document_name

    # --- Chunking (caracteres) ---
    chunk_size_min: int = 800
    chunk_size_max: int = 1200
    chunk_overlap: int = 200

    # --- Recuperación ---
    retrieval_k: int = 6  # entre 5 y 8 fragmentos por consulta

    # --- Servidor ---
    host: str = os.getenv("HOST", "0.0.0.0").strip()
    port: int = int(os.getenv("PORT", "8000"))

    @property
    def claude_is_configured(self) -> bool:
        """True cuando existe una API key real de Anthropic en el entorno."""
        return is_usable_anthropic_key(self.anthropic_api_key)


settings = Settings()
