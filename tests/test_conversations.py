"""Pruebas del historial de conversaciones y del reinicio tras cerrar un caso."""

from __future__ import annotations

from pathlib import Path
from unittest.mock import AsyncMock

import pytest
from fastapi.testclient import TestClient

from app.services.case_service import CaseRecord, CaseService
from app.services.claude_service import ClaudeService, SupportTurn
from app.services.conversation_service import ConversationService
from app.services.document_service import DocumentService
from app.services.rag_service import RagService
from tests.conftest import make_vector_service, write_sample_document


def test_record_turn_persists_full_transcript(tmp_path: Path) -> None:
    store = ConversationService(store_dir=tmp_path / "conversations")
    store.record_turn(
        session_id="ses-demo-abc123",
        question="Hola, mi correo es ana@empresa.co",
        answer="Gracias, Ana. ¿El tema es correo, dominio, hosting o facturación?",
        history=[
            {
                "role": "assistant",
                "content": "¿Me compartes el correo con el que estás registrado?",
            }
        ],
    )
    stored = store.get("ses-demo-abc123")
    assert stored is not None
    assert stored.status == "open"
    assert len(stored.messages) == 3
    assert stored.messages[1]["content"].startswith("Hola")
    listed = store.list_conversations()
    assert listed[0].session_id == "ses-demo-abc123"
    assert "ana@empresa.co" in listed[0].summary


def test_closed_session_opens_a_new_chat(tmp_path: Path) -> None:
    store = ConversationService(store_dir=tmp_path / "conversations")
    case = CaseRecord(
        case_id="CASO-1",
        correo="ana@empresa.co",
        categoria="Hosting",
        detalle="No entra a cPanel",
        registrado_en="2026-08-25 09:00:00 COT",
        estado="Cerrado/Registrado",
        session_id="ses-old-closed1",
    )
    store.record_turn(
        session_id="ses-old-closed1",
        question="Sí, confirma el caso",
        answer="Quedó registrado. Hasta pronto.",
        case=case,
    )
    closed = store.get("ses-old-closed1")
    assert closed is not None
    assert closed.status == "closed"

    session_id, started_new = store.resolve_session("ses-old-closed1")
    assert started_new is True
    assert session_id != "ses-old-closed1"


@pytest.mark.asyncio
async def test_ask_starts_new_chat_when_previous_case_is_closed(tmp_path: Path) -> None:
    path = write_sample_document(tmp_path / "documento.txt")
    chunks, _characters, fingerprint = DocumentService(document_path=path).process()
    vector_service = make_vector_service(tmp_path)
    vector_service.index_chunks(chunks, fingerprint, reset=True)

    conversations = ConversationService(store_dir=tmp_path / "conversations")
    conversations.record_turn(
        session_id="ses-closed-xyz789",
        question="Confirma por favor",
        answer="Caso cerrado. Gracias.",
        case=CaseRecord(
            case_id="CASO-99",
            correo="luis@mi.com.co",
            categoria="Dominio",
            detalle="El dominio no abre",
            registrado_en="2026-08-25 10:00:00 COT",
            estado="Cerrado/Registrado",
            session_id="ses-closed-xyz789",
        ),
    )

    claude = ClaudeService(api_key="test-key-not-real")
    claude.generate_support_reply = AsyncMock(  # type: ignore[method-assign]
        return_value=SupportTurn(
            text="Con gusto. Para ubicar tu cuenta, ¿me compartes el correo con el que estás registrado?"
        )
    )
    rag = RagService(
        document_service=DocumentService(document_path=path),
        vector_service=vector_service,
        claude_service=claude,
        case_service=CaseService(store_path=tmp_path / "casos.jsonl"),
        conversation_service=conversations,
    )
    response = await rag.ask(
        "Hola, otra vez necesito ayuda",
        history=[{"role": "user", "content": "este historial viejo no debe usarse"}],
        session_id="ses-closed-xyz789",
    )
    assert response.new_chat is True
    assert response.session_id != "ses-closed-xyz789"
    assert response.case_closed is False
    claude.generate_support_reply.assert_awaited()
    sent_history = claude.generate_support_reply.await_args.kwargs["history"]
    assert sent_history == []
    fresh = conversations.get(response.session_id)
    assert fresh is not None
    assert fresh.status == "open"
    assert conversations.get("ses-closed-xyz789").status == "closed"


def test_conversations_api_lists_and_reads(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    store_dir = tmp_path / "conversations"
    store = ConversationService(store_dir=store_dir)
    store.record_turn(
        session_id="ses-api-read001",
        question="Mi correo es ana@empresa.co",
        answer="Gracias, ¿en qué te ayudo?",
    )

    from app.main import app
    from app import main as main_module

    write_sample_document(tmp_path / "documento.txt")

    def fake_rag():
        return RagService(
            document_service=DocumentService(document_path=tmp_path / "documento.txt"),
            vector_service=make_vector_service(tmp_path),
            claude_service=ClaudeService(api_key=""),
            case_service=CaseService(store_path=tmp_path / "casos.jsonl"),
            conversation_service=store,
        )

    monkeypatch.setattr(main_module, "_build_rag_service", fake_rag)

    with TestClient(app) as client:
        listed = client.get("/api/conversations")
        assert listed.status_code == 200
        payload = listed.json()
        assert payload[0]["session_id"] == "ses-api-read001"
        detail = client.get("/api/conversations/ses-api-read001")
        assert detail.status_code == 200
        body = detail.json()
        assert len(body["messages"]) == 2
        missing = client.get("/api/conversations/ses-no-existe-xyz")
        assert missing.status_code == 404
