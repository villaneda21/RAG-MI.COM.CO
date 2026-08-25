"""Pruebas de indexación de varios archivos de conocimiento."""

from __future__ import annotations

from pathlib import Path

from app.services.document_service import DocumentService, list_knowledge_files
from app.services.rag_service import RagService
from app.services.claude_service import ClaudeService
from tests.conftest import make_vector_service, write_sample_document


def test_list_knowledge_files_skips_templates(tmp_path: Path) -> None:
    write_sample_document(tmp_path / "documento.txt")
    (tmp_path / "_plantilla_guia.txt").write_text("no indexar", encoding="utf-8")
    (tmp_path / "README.md").write_text("# hola", encoding="utf-8")
    (tmp_path / "guia_nueva.md").write_text("Cómo renovar un dominio paso a paso.", encoding="utf-8")
    names = [path.name for path in list_knowledge_files(tmp_path)]
    assert "documento.txt" in names
    assert "guia_nueva.md" in names
    assert "_plantilla_guia.txt" not in names
    assert "README.md" not in names


def test_reindex_adds_a_new_file_without_wiping_the_first(tmp_path: Path) -> None:
    write_sample_document(tmp_path / "documento.txt")
    rag = RagService(
        document_service=DocumentService(document_path=tmp_path / "documento.txt"),
        vector_service=make_vector_service(tmp_path),
        claude_service=ClaudeService(api_key=""),
    )
    first = rag.reindex(reset=True)
    assert first.files_indexed == 1
    first_count = rag.vector_service.count()

    (tmp_path / "renovacion.txt").write_text(
        "Para renovar un dominio entra a Mis servicios y revisa la fecha de vencimiento.",
        encoding="utf-8",
    )
    second = rag.reindex(reset=False)
    assert second.files_indexed == 1
    assert "renovacion.txt" in second.sources
    assert rag.vector_service.count() > first_count

    third = rag.reindex(reset=False)
    assert third.files_indexed == 0
    assert third.files_skipped == 2


def test_index_status_detects_new_and_changed_files(tmp_path: Path) -> None:
    write_sample_document(tmp_path / "documento.txt")
    rag = RagService(
        document_service=DocumentService(document_path=tmp_path / "documento.txt"),
        vector_service=make_vector_service(tmp_path),
        claude_service=ClaudeService(api_key=""),
    )
    rag.reindex(reset=True)
    (tmp_path / "extra.txt").write_text("Nueva guía de facturación electrónica.", encoding="utf-8")
    (tmp_path / "documento.txt").write_text(
        "El código de cliente tiene el formato CLI-XXXXXX y se consulta en Mi cuenta. " * 8,
        encoding="utf-8",
    )
    status = rag.index_status()
    by_name = {item.name: item.status for item in status.files}
    assert by_name["extra.txt"] == "new"
    assert by_name["documento.txt"] == "changed"
    assert status.pending_changes is True


def test_index_status_endpoint_hides_templates() -> None:
    from app.main import app
    from fastapi.testclient import TestClient

    with TestClient(app) as client:
        response = client.get("/api/index")
        assert response.status_code == 200
        names = [item["name"] for item in response.json()["files"]]
        assert all(not name.startswith("_") for name in names)
