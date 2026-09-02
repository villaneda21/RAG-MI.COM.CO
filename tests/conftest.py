"""
Asistente virtual MI.COM.CO
Identidad visual inspirada en el sitio y el portal de mi.com.co:
azul primario #0D6EFD, acero #86A1AC, superficies oscuras del panel interno
y tipografías DM Sans / Plus Jakarta Sans.
"""

from __future__ import annotations

import hashlib
from pathlib import Path
from typing import Sequence

class FakeEmbeddingFunction:
    """Embeddings deterministas para pruebas (sin descargar modelos)."""

    def __init__(self, dimensions: int = 32) -> None:
        self.dimensions = dimensions

    def name(self) -> str:
        return "fake-deterministic"

    def __call__(self, input):
        vectors: list[list[float]] = []
        for text in input:
            digest = hashlib.sha256(text.encode("utf-8")).digest()
            values = [(digest[i % len(digest)] / 255.0) for i in range(self.dimensions)]
            # Pequeña variación por longitud para que textos distintos no colisionen tanto.
            values[0] = (values[0] + (len(text) % 50) / 50.0) / 2.0
            vectors.append(values)
        return vectors


def write_sample_document(path: Path, paragraphs: Sequence[str] | None = None) -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    content = "\n\n".join(
        paragraphs
        or [
            "La política de la empresa indica que el soporte técnico responde en horario continuo.",
            "El código de cliente tiene el formato CLI-XXXXXX y se consulta en Mi cuenta.",
            "Para acceder a cPanel se debe entrar a Mis servicios y pulsar Acceder a cPanel.",
        ]
    )
    path.write_text(content, encoding="utf-8")
    return path


def make_vector_service(tmp_path: Path):
    from app.services.vector_service import VectorService

    persist = tmp_path / "chroma_db"
    meta = persist / ".index_meta.json"
    return VectorService(
        persist_directory=persist,
        collection_name="test_knowledge_base",
        embedding_factory=FakeEmbeddingFunction,
        meta_path=meta,
    )
