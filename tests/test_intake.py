"""Pruebas del flujo de atención, orientación con la base y registro de casos."""

from __future__ import annotations

from pathlib import Path
from unittest.mock import AsyncMock

import pytest
from fastapi.testclient import TestClient

from app.models.schemas import ChatResponse, ChatTurn, IntakeState
from app.services.case_store import CaseStore
from app.services.claude_service import ClaudeService
from app.services.document_service import DocumentService
from app.services.intake_service import (
    classify_category,
    continue_intake,
    is_connection_restricted,
    wants_case_flow,
)
from app.services.rag_service import RagService
from tests.conftest import make_vector_service, write_sample_document


def test_detects_case_request() -> None:
    assert wants_case_flow("Quiero comunicarme con un asesor humano")
    assert wants_case_flow("pásame con un asesor")
    assert wants_case_flow("Quiero abrir un caso")
    assert not wants_case_flow("¿Cómo creo una cuenta en mi.com.co?")
    assert not wants_case_flow("¿Qué es la asesoría de configuración de Google Apps?")


def test_classifies_help_types() -> None:
    assert classify_category("No me llega el correo, necesito un asesor") == "correo"
    assert classify_category("El dominio está por vencer, quiero un asesor") == "dominio"
    assert classify_category("No puedo entrar a cPanel de mi hosting") == "hosting"
    assert classify_category("Mi factura está vencida, conectame con un asesor") == "facturacion"
    assert classify_category("Quiero cotizar un plan nuevo") == "facturacion"


def test_blocks_connection_diagnostics() -> None:
    assert is_connection_restricted("¿Puedes revisar mis registros SPF y DKIM?")
    assert is_connection_restricted("haz ping al servidor y mira el DNS")
    assert not is_connection_restricted("¿Cómo creo un buzón de correo?")
    assert not is_connection_restricted("¿Cómo entro a cPanel?")


def test_case_flow_asks_for_email_first() -> None:
    decision = continue_intake("Quiero comunicarme con un asesor humano")
    assert decision.handled is True
    assert decision.skip_rag is True
    assert decision.snapshot.step == "awaiting_email"
    assert "correo" in decision.answer.lower()


def test_case_flow_then_asks_reason(tmp_path: Path) -> None:
    first = continue_intake("Quiero un asesor humano")
    second = continue_intake(
        "ana@empresa.com",
        current=first.snapshot,
        store=CaseStore(tmp_path / "casos.json"),
    )
    assert second.snapshot.email == "ana@empresa.com"
    assert second.snapshot.step == "awaiting_reason"
    assert second.skip_rag is True


def test_connection_topic_is_refused_during_case(tmp_path: Path) -> None:
    store = CaseStore(tmp_path / "casos.json")
    start = continue_intake("Quiero un asesor, mi correo es ana@empresa.com y no me llega el mail")
    refused = continue_intake(
        "Revisa mi SPF y haz ping al servidor",
        current=start.snapshot,
        store=store,
    )
    assert refused.skip_rag is True
    lower = refused.answer.lower()
    assert "dns" in lower or "spf" in lower or "ping" in lower
    assert "no" in lower
    assert "nslookup" not in lower


@pytest.mark.asyncio
async def test_case_flow_uses_knowledge_base_then_closes(tmp_path: Path) -> None:
    path = write_sample_document(tmp_path / "documento.txt")
    chunks, _characters, fingerprint = DocumentService(document_path=path).process()
    vector_service = make_vector_service(tmp_path)
    vector_service.index_chunks(chunks, fingerprint, reset=True)

    claude = ClaudeService(api_key="test-key-not-real")
    claude.generate_response = AsyncMock(  # type: ignore[method-assign]
        return_value="Para el correo corporativo entra al panel y crea el buzón desde Correo."
    )
    store = CaseStore(tmp_path / "casos.json")
    rag = RagService(
        document_service=DocumentService(document_path=path),
        vector_service=vector_service,
        claude_service=claude,
        case_store=store,
    )

    first = await rag.ask("Quiero un asesor, soy ana@empresa.com y no me llega el correo")
    assert first.intake is not None
    assert first.intake.email == "ana@empresa.com"
    assert first.intake.category == "correo"
    assert first.intake.needs_close is True
    assert "buzón" in first.answer.lower() or "bizon" in first.answer.lower() or "correo" in first.answer.lower()
    claude.generate_response.assert_awaited()

    closed = await rag.ask(
        "Sí, registra el caso",
        intake=first.intake,
        history=[
            ChatTurn(role="user", content="Quiero un asesor, soy ana@empresa.com y no me llega el correo"),
            ChatTurn(role="assistant", content=first.answer),
        ],
    )
    assert closed.case is not None
    assert closed.case.status == "Cerrado"
    assert closed.case.email == "ana@empresa.com"
    assert closed.case.help_type == "correo"
    assert closed.intake is not None
    assert closed.intake.registered is True
    saved = store.list_by_email("ana@empresa.com")
    assert len(saved) == 1
    assert saved[0]["status"] == "Cerrado"


@pytest.mark.asyncio
async def test_regular_knowledge_question_still_uses_rag(tmp_path: Path) -> None:
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
        case_store=CaseStore(tmp_path / "casos.json"),
    )
    response = await rag.ask("¿Cuál es la política de soporte?")
    assert isinstance(response, ChatResponse)
    assert response.intake is None
    assert "soporte" in response.answer.lower()
    claude.generate_response.assert_awaited()


def test_chat_api_starts_case_without_claude() -> None:
    from app.main import app

    with TestClient(app) as client:
        response = client.post(
            "/api/chat",
            json={"question": "Quiero comunicarme con un asesor humano"},
        )
        assert response.status_code == 200
        payload = response.json()
        assert payload["intake"]["active"] is True
        assert payload["intake"]["needs_email"] is True
        assert "correo" in payload["answer"].lower()


def test_global_connection_question_is_refused() -> None:
    from app.main import app

    with TestClient(app) as client:
        response = client.post(
            "/api/chat",
            json={"question": "Haz ping al servidor y revisa el registro SPF"},
        )
        assert response.status_code == 200
        text = response.json()["answer"].lower()
        assert "spf" in text or "dns" in text or "ping" in text
        assert "no hago" in text or "no diagnostic" in text or "no hago cambios" in text


def test_home_mentions_advisor() -> None:
    from app.main import app

    with TestClient(app) as client:
        response = client.get("/")
        assert response.status_code == 200
        assert "asesor" in response.text
        assert "status-label" in response.text


def test_frontend_keeps_status_label_binding() -> None:
    js = Path(__file__).resolve().parents[1] / "static" / "js" / "app.js"
    source = js.read_text(encoding="utf-8")
    assert 'getElementById("status-label")' in source
    assert "intakeState" in source
    assert "requested_category" in source
