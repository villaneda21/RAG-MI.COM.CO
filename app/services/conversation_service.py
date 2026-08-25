"""Persistencia del historial completo de cada conversación de soporte."""

from __future__ import annotations

import json
import logging
import re
import uuid
from dataclasses import asdict, dataclass, field
from datetime import datetime
from pathlib import Path
from typing import Any

from app.config.settings import settings
from app.services.case_service import colombia_tz

logger = logging.getLogger(__name__)

SAFE_SESSION_ID = re.compile(r"^[A-Za-z0-9._-]{6,96}$")


def new_session_id() -> str:
    stamp = datetime.now(colombia_tz()).strftime("%Y%m%d%H%M%S")
    return f"ses-{stamp}-{uuid.uuid4().hex[:8]}"


def is_safe_session_id(value: str | None) -> bool:
    raw = (value or "").strip()
    return bool(SAFE_SESSION_ID.fullmatch(raw)) and Path(raw).name == raw


def _now_stamp() -> str:
    return datetime.now(colombia_tz()).strftime("%Y-%m-%d %H:%M:%S %Z")


@dataclass
class ConversationRecord:
    """Chat completo guardado en disco."""

    session_id: str
    created_at: str
    updated_at: str
    status: str = "open"
    case_id: str = ""
    correo: str = ""
    categoria: str = ""
    summary: str = ""
    messages: list[dict[str, Any]] = field(default_factory=list)

    def to_dict(self) -> dict[str, Any]:
        return asdict(self)

    def to_list_item(self) -> dict[str, Any]:
        payload = self.to_dict()
        payload["message_count"] = len(self.messages)
        payload.pop("messages", None)
        return payload


class ConversationService:
    """Guarda cada turno en JSON, un archivo por sesión."""

    def __init__(self, store_dir: Path | None = None) -> None:
        self.store_dir = Path(store_dir or settings.conversations_directory)

    def resolve_session(self, requested: str | None) -> tuple[str, bool]:
        """Devuelve un session_id activo. Si el chat ya se cerró, abre uno nuevo."""
        if is_safe_session_id(requested):
            existing = self.get(requested)
            if existing is None or existing.status != "closed":
                return requested, False
        return new_session_id(), True

    def get(self, session_id: str) -> ConversationRecord | None:
        path = self._path_for(session_id)
        if path is None or not path.exists():
            return None
        try:
            payload = json.loads(path.read_text(encoding="utf-8"))
        except (json.JSONDecodeError, OSError):
            logger.warning("No se pudo leer la conversación %s.", session_id)
            return None
        try:
            return ConversationRecord(
                session_id=str(payload.get("session_id") or session_id),
                created_at=str(payload.get("created_at") or ""),
                updated_at=str(payload.get("updated_at") or ""),
                status=str(payload.get("status") or "open"),
                case_id=str(payload.get("case_id") or ""),
                correo=str(payload.get("correo") or ""),
                categoria=str(payload.get("categoria") or ""),
                summary=str(payload.get("summary") or ""),
                messages=list(payload.get("messages") or []),
            )
        except TypeError:
            return None

    def record_turn(
        self,
        session_id: str,
        question: str,
        answer: str,
        history: list[dict[str, str]] | None = None,
        case: Any | None = None,
    ) -> ConversationRecord:
        """Añade el turno al archivo de la sesión y cierra si hay caso."""
        now = _now_stamp()
        record = self.get(session_id)
        if record is None:
            record = ConversationRecord(
                session_id=session_id,
                created_at=now,
                updated_at=now,
                messages=_seed_messages(history or [], now),
            )

        record.messages.append({"role": "user", "content": question, "created_at": now})
        record.messages.append({"role": "assistant", "content": answer, "created_at": now})
        record.updated_at = now

        if case is not None:
            record.status = "closed"
            record.case_id = getattr(case, "case_id", "") or ""
            record.correo = getattr(case, "correo", "") or ""
            record.categoria = getattr(case, "categoria", "") or ""

        record.summary = _build_summary(record)
        self._write(record)
        logger.info(
            "Conversación %s actualizada (%s, %s mensajes).",
            record.session_id,
            record.status,
            len(record.messages),
        )
        return record

    def list_conversations(self, limit: int = 50) -> list[ConversationRecord]:
        self.store_dir.mkdir(parents=True, exist_ok=True)
        records: list[ConversationRecord] = []
        for path in self.store_dir.glob("*.json"):
            record = self.get(path.stem)
            if record is not None:
                records.append(record)
        records.sort(key=lambda item: item.updated_at, reverse=True)
        return records[: max(1, min(limit, 200))]

    def _write(self, record: ConversationRecord) -> None:
        path = self._path_for(record.session_id)
        if path is None:
            raise ValueError("session_id inválido para guardar la conversación.")
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(
            json.dumps(record.to_dict(), ensure_ascii=False, indent=2),
            encoding="utf-8",
        )

    def _path_for(self, session_id: str) -> Path | None:
        if not is_safe_session_id(session_id):
            return None
        return self.store_dir / f"{session_id}.json"


def _seed_messages(history: list[dict[str, str]], stamp: str) -> list[dict[str, Any]]:
    seeded: list[dict[str, Any]] = []
    for item in history:
        role = (item.get("role") or "").strip()
        content = (item.get("content") or "").strip()
        if role not in {"user", "assistant"} or not content:
            continue
        seeded.append({"role": role, "content": content, "created_at": stamp})
    return seeded


def _build_summary(record: ConversationRecord) -> str:
    if record.categoria and record.correo:
        return f"{record.categoria} · {record.correo}"
    if record.categoria:
        return record.categoria
    for message in record.messages:
        if message.get("role") == "user":
            text = " ".join(str(message.get("content") or "").split())
            if text:
                return text[:140] + ("…" if len(text) > 140 else "")
    return "Conversación de soporte"
