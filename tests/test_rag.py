"""Pruebas del pipeline RAG: lectura, chunks, ChromaDB, consulta y contexto.

Las pruebas que requerirían una API Key real de Anthropic se omiten
o se ejecutan con un doble (mock) de ClaudeService.
"""

from __future__ import annotations

from pathlib import Path
from unittest.mock import AsyncMock

import pytest
from fastapi.testclient import TestClient

from app.models.schemas import ChatResponse
from app.services.claude_service import ClaudeService
from app.services.document_service import DocumentService
from app.services.rag_service import RagService
from app.utils.text_processor import (
    DocumentProcessingError,
    clean_text,
    parse_structured_document,
    split_into_chunks,
)
from tests.conftest import FakeEmbeddingFunction, make_vector_service, write_sample_document

SAMPLE_STRUCTURED = """
================================================================================
[CHUNK_ID]: GUA-001
[CATEGORY]: Guías Operativas
[TITLE]: Cómo crear tu cuenta
[SUMMARY]: Pasos para registrarte.
--------------------------------------------------------------------------------
[CONTENT]:
Accede a mi.com.co y haz clic en Crear Cuenta. Completa nombre, email y contraseña.
Verifica el correo electrónico para activar el acceso.
================================================================================

================================================================================
[CHUNK_ID]: GUA-002
[CATEGORY]: Guías Operativas
[TITLE]: Código de cliente
[SUMMARY]: Dónde encontrar el identificador CLI.
--------------------------------------------------------------------------------
[CONTENT]:
El Código de Cliente tiene el formato CLI-XXXXXX. Entra a Mi cuenta o Perfil
y ubica el campo CÓDIGO DE CLIENTE en la columna derecha.
================================================================================
"""


def test_read_txt(tmp_path: Path) -> None:
    path = write_sample_document(tmp_path / "documento.txt")
    service = DocumentService(document_path=path)
    raw = service.load_raw_text()
    assert "política de la empresa" in raw
    assert len(raw) > 0


def test_empty_txt_raises(tmp_path: Path) -> None:
    path = tmp_path / "vacio.txt"
    path.write_text("   \n", encoding="utf-8")
    service = DocumentService(document_path=path)
    with pytest.raises(DocumentProcessingError):
        service.load_raw_text()


def test_missing_txt_raises(tmp_path: Path) -> None:
    service = DocumentService(document_path=tmp_path / "no-existe.txt")
    with pytest.raises(DocumentProcessingError):
        service.load_raw_text()


def test_generate_chunks() -> None:
    text = (
        "Párrafo uno sobre dominios .co y registro en Colombia. " * 20
        + "\n\n"
        + "Párrafo dos sobre hosting, cPanel y certificados SSL. " * 20
        + "\n\n"
        + "Párrafo tres sobre correo corporativo y webmail. " * 20
    )
    chunks = split_into_chunks(text, source="documento.txt")
    assert len(chunks) >= 2
    assert all(chunk.content.strip() for chunk in chunks)
    assert all(chunk.metadata.get("source") == "documento.txt" for chunk in chunks)
    assert [chunk.chunk_id for chunk in chunks] == list(range(1, len(chunks) + 1))


def test_structured_chunks_keep_metadata() -> None:
    chunks = parse_structured_document(SAMPLE_STRUCTURED, source="documento.txt")
    assert chunks is not None
    assert len(chunks) == 2
    assert chunks[0].metadata["doc_chunk_id"] == "GUA-001"
    assert chunks[0].metadata["title"] == "Cómo crear tu cuenta"
    assert "Crear Cuenta" in chunks[0].content


def test_clean_text_keeps_meaning() -> None:
    messy = "Hola   mundo\r\n\r\n\r\nSiguiente   párrafo.\t"
    cleaned = clean_text(messy)
    assert "Hola mundo" in cleaned
    assert "Siguiente párrafo." in cleaned
    assert "\r" not in cleaned


def test_chromadb_stores_information(tmp_path: Path) -> None:
    path = write_sample_document(tmp_path / "documento.txt")
    document_service = DocumentService(document_path=path)
    chunks, _characters, fingerprint = document_service.process()
    vector_service = make_vector_service(tmp_path)

    stored = vector_service.index_chunks(chunks, fingerprint, reset=True)
    assert stored == len(chunks)
    assert vector_service.count() == len(chunks)

    # Segunda ingestión con el mismo hash no duplica.
    stored_again = vector_service.index_chunks(chunks, fingerprint, reset=False)
    assert vector_service.count() == stored_again
    assert vector_service.count() == len(chunks)


def test_query_returns_results(tmp_path: Path) -> None:
    path = write_sample_document(tmp_path / "documento.txt")
    chunks, _characters, fingerprint = DocumentService(document_path=path).process()
    vector_service = make_vector_service(tmp_path)
    vector_service.index_chunks(chunks, fingerprint, reset=True)

    hits = vector_service.query("¿Dónde veo el código de cliente?", n_results=3)
    assert len(hits) >= 1
    assert all("document" in hit for hit in hits)
    assert all("distance" in hit for hit in hits)
    joined = " ".join(hit["document"] for hit in hits)
    assert "CLI-XXXXXX" in joined or "cPanel" in joined or "soporte" in joined.lower()


def test_build_context(tmp_path: Path) -> None:
    path = write_sample_document(tmp_path / "documento.txt")
    chunks, _characters, fingerprint = DocumentService(document_path=path).process()
    vector_service = make_vector_service(tmp_path)
    vector_service.index_chunks(chunks, fingerprint, reset=True)
    hits = vector_service.query("soporte técnico", n_results=5)

    rag = RagService(vector_service=vector_service, claude_service=ClaudeService(api_key=""))
    context = rag.build_context(hits)
    assert "[Chunk 1" in context
    assert "fuente=" in context
    assert len(context) > 40


@pytest.mark.asyncio
async def test_ask_uses_mock_claude(tmp_path: Path) -> None:
    path = write_sample_document(tmp_path / "documento.txt")
    chunks, _characters, fingerprint = DocumentService(document_path=path).process()
    vector_service = make_vector_service(tmp_path)
    vector_service.index_chunks(chunks, fingerprint, reset=True)

    claude = ClaudeService(api_key="test-key-not-real")
    claude.generate_response = AsyncMock(return_value="El soporte responde en horario continuo.")  # type: ignore[method-assign]

    rag = RagService(
        document_service=DocumentService(document_path=path),
        vector_service=vector_service,
        claude_service=claude,
    )
    response = await rag.ask("¿Cuál es la política de soporte?")
    assert isinstance(response, ChatResponse)
    assert "soporte" in response.answer.lower()
    assert response.sources
    claude.generate_response.assert_awaited()


def test_chat_requires_question() -> None:
    from app.main import app

    with TestClient(app) as client:
        response = client.post("/api/chat", json={"question": "   "})
        assert response.status_code == 422


def test_health_endpoint() -> None:
    from app.main import app

    with TestClient(app) as client:
        response = client.get("/health")
        assert response.status_code == 200
        payload = response.json()
        assert "status" in payload
        assert "vector_database" in payload


def test_system_prompt_keeps_knowledge_and_blocks_connections() -> None:
    from app.services.claude_service import SYSTEM_PROMPT

    assert "base de conocimiento" in SYSTEM_PROMPT.lower()
    assert "dns" in SYSTEM_PROMPT.lower()
    assert "spf" in SYSTEM_PROMPT.lower()
    assert "Markdown" in SYSTEM_PROMPT
    assert "asesor" in SYSTEM_PROMPT.lower()


def test_home_renders_chatbot() -> None:
    from app.main import app

    with TestClient(app) as client:
        response = client.get("/")
        assert response.status_code == 200
        assert "Asistente" in response.text
        assert "/static/css/style.css" in response.text


def test_reindex_with_isolated_services(tmp_path: Path) -> None:
    path = write_sample_document(tmp_path / "documento.txt")
    rag = RagService(
        document_service=DocumentService(document_path=path),
        vector_service=make_vector_service(tmp_path),
        claude_service=ClaudeService(api_key=""),
    )
    result = rag.reindex(reset=True)
    assert result.status == "ok"
    assert result.chunks_indexed >= 1
    assert rag.vector_service.count() == result.chunks_indexed


def test_fake_embedding_function_shape() -> None:
    fn = FakeEmbeddingFunction(dimensions=16)
    vectors = fn(["hola", "mundo"])
    assert len(vectors) == 2
    assert len(vectors[0]) == 16
    assert vectors[0] != vectors[1]
