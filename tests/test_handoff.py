"""Pruebas de conexión con asesor humano y ruteo por área."""

from __future__ import annotations

from pathlib import Path
from unittest.mock import AsyncMock

import pytest
from fastapi.testclient import TestClient

from app.models.schemas import ChatResponse, ChatTurn
from app.services.claude_service import ClaudeService
from app.services.document_service import DocumentService
from app.services.handoff_service import classify_area, resolve_handoff, wants_human_advisor
from app.services.rag_service import RagService
from tests.conftest import make_vector_service, write_sample_document


def test_detects_human_advisor_request() -> None:
    assert wants_human_advisor("Quiero comunicarme con un asesor humano")
    assert wants_human_advisor("pásame con un asesor")
    assert wants_human_advisor("Necesito hablar con una persona real")
    assert not wants_human_advisor("¿Cómo creo una cuenta en mi.com.co?")
    assert not wants_human_advisor("¿Qué es la asesoría de configuración de Google Apps?")


def test_classifies_area_from_the_same_message() -> None:
    assert classify_area("No me llega el correo, necesito un asesor") == "correo"
    assert classify_area("El dominio no resuelve, quiero un asesor humano") == "hdr"
    assert classify_area("Mi factura está vencida, conectame con un asesor") == "facturacion"
    assert classify_area("Quiero cotizar un plan nuevo, un asesor de ventas") == "ventas"


def test_classifies_area_from_history() -> None:
    history = ["No puedo entrar a cPanel de mi hosting"]
    assert classify_area("Quiero un asesor humano", history=history) == "hdr"


def test_explicit_area_selection() -> None:
    assert classify_area("Asignar al área de Facturación") == "facturacion"
    assert classify_area("cualquier cosa", requested_area="correo") == "correo"


def test_handoff_connects_and_assigns_correo() -> None:
    decision = resolve_handoff("No me funciona el webmail, necesito un asesor humano")
    assert decision.requested is True
    assert decision.connected is True
    assert decision.area_id == "correo"
    assert "asignada al área de Correo" in decision.answer


def test_handoff_connects_hdr() -> None:
    decision = resolve_handoff("Quiero hablar con un asesor de hosting y dominios")
    assert decision.connected is True
    assert decision.area_id == "hdr"
    assert "HDR (Dominio y Hosting)" in decision.answer


def test_handoff_asks_for_area_when_unclear() -> None:
    decision = resolve_handoff("Quiero comunicarme con un asesor humano")
    assert decision.requested is True
    assert decision.connected is False
    assert decision.needs_area is True
    assert decision.area is None
    assert "Correo" in decision.answer
    assert "HDR" in decision.answer
    assert "Facturación" in decision.answer
    assert "Ventas" in decision.answer


def test_handoff_uses_clicked_area() -> None:
    decision = resolve_handoff(
        "Asignar al área de Ventas",
        requested_area="ventas",
    )
    assert decision.connected is True
    assert decision.area_id == "ventas"
    assert "área de Ventas" in decision.answer


def test_queued_message_after_assignment() -> None:
    decision = resolve_handoff(
        "Sigo sin poder enviar correos desde Outlook",
        handed_off_area="correo",
    )
    assert decision.queued_message is True
    assert decision.connected is True
    assert decision.area_id == "correo"
    assert "envió al" in decision.answer


@pytest.mark.asyncio
async def test_ask_handoff_skips_claude(tmp_path: Path) -> None:
    path = write_sample_document(tmp_path / "documento.txt")
    chunks, _characters, fingerprint = DocumentService(document_path=path).process()
    vector_service = make_vector_service(tmp_path)
    vector_service.index_chunks(chunks, fingerprint, reset=True)

    claude = ClaudeService(api_key="test-key-not-real")
    claude.generate_response = AsyncMock(return_value="no debería llamarse")  # type: ignore[method-assign]

    rag = RagService(
        document_service=DocumentService(document_path=path),
        vector_service=vector_service,
        claude_service=claude,
    )
    response = await rag.ask("Quiero un asesor de facturación")
    assert isinstance(response, ChatResponse)
    assert response.handoff is not None
    assert response.handoff.connected is True
    assert response.handoff.area == "facturacion"
    assert "Facturación" in response.answer
    claude.generate_response.assert_not_awaited()


@pytest.mark.asyncio
async def test_ask_handoff_uses_conversation_history(tmp_path: Path) -> None:
    rag = RagService(claude_service=ClaudeService(api_key=""))
    response = await rag.ask(
        "Quiero hablar con un asesor humano",
        history=[ChatTurn(role="user", content="Mi dominio .co no apunta al hosting")],
    )
    assert response.handoff is not None
    assert response.handoff.area == "hdr"
    assert response.handoff.connected is True


def test_chat_api_returns_handoff_payload() -> None:
    from app.main import app

    with TestClient(app) as client:
        response = client.post(
            "/api/chat",
            json={"question": "Necesito un asesor humano de correo"},
        )
        assert response.status_code == 200
        payload = response.json()
        assert payload["handoff"]["requested"] is True
        assert payload["handoff"]["connected"] is True
        assert payload["handoff"]["area"] == "correo"
        assert "asignada al área de Correo" in payload["answer"]


def test_home_mentions_human_advisor() -> None:
    from app.main import app

    with TestClient(app) as client:
        response = client.get("/")
        assert response.status_code == 200
        assert "asesor humano" in response.text
        assert "status-label" in response.text


def test_frontend_keeps_status_label_binding() -> None:
    from pathlib import Path

    js = Path(__file__).resolve().parents[1] / "static" / "js" / "app.js"
    source = js.read_text(encoding="utf-8")
    assert 'getElementById("status-label")' in source
    assert 'getElementById("composer-hint")' in source
