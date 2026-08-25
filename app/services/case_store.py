"""Almacenamiento persistente del historial de casos de atención."""

from __future__ import annotations

import json
import logging
import threading
from datetime import datetime
from pathlib import Path
from typing import Any
from zoneinfo import ZoneInfo

from app.config.settings import PROJECT_ROOT

logger = logging.getLogger(__name__)

BOGOTA = ZoneInfo("America/Bogota")
CASE_STATUS_CLOSED = "Cerrado"


def now_bogota() -> datetime:
    return datetime.now(BOGOTA)


def format_registered_at(moment: datetime | None = None) -> tuple[str, str]:
    """ISO y texto amigable en hora de Colombia."""
    stamp = moment or now_bogota()
    iso = stamp.isoformat(timespec="seconds")
    display = stamp.strftime("%d/%m/%Y, %I:%M %p").replace("AM", "a. m.").replace("PM", "p. m.")
    return iso, f"{display} (Colombia)"


class CaseStore:
    """Guarda casos cerrados en un JSON local, con candado para escrituras concurrentes."""

    def __init__(self, path: Path | None = None) -> None:
        self.path = path or (PROJECT_ROOT / "data" / "cases" / "casos.json")
        self._lock = threading.Lock()

    def _read(self) -> list[dict[str, Any]]:
        if not self.path.exists():
            return []
        try:
            payload = json.loads(self.path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError):
            logger.exception("No se pudo leer el historial de casos.")
            return []
        if isinstance(payload, list):
            return payload
        if isinstance(payload, dict) and isinstance(payload.get("cases"), list):
            return payload["cases"]
        return []

    def _write(self, records: list[dict[str, Any]]) -> None:
        self.path.parent.mkdir(parents=True, exist_ok=True)
        tmp = self.path.with_suffix(".tmp")
        tmp.write_text(
            json.dumps({"cases": records}, ensure_ascii=False, indent=2),
            encoding="utf-8",
        )
        tmp.replace(self.path)

    def next_case_id(self, records: list[dict[str, Any]] | None = None) -> str:
        items = records if records is not None else self._read()
        sequential = len(items) + 1
        day = now_bogota().strftime("%Y%m%d")
        return f"CAS-{day}-{sequential:04d}"

    def save(
        self,
        *,
        email: str,
        help_type: str,
        help_type_label: str,
        summary: str,
        status: str = CASE_STATUS_CLOSED,
    ) -> dict[str, Any]:
        iso, display = format_registered_at()
        with self._lock:
            records = self._read()
            record = {
                "case_id": self.next_case_id(records),
                "email": email.strip().lower(),
                "help_type": help_type,
                "help_type_label": help_type_label,
                "summary": (summary or "").strip(),
                "registered_at": iso,
                "registered_at_display": display,
                "status": status,
            }
            records.append(record)
            self._write(records)
        logger.info("Caso %s registrado (%s).", record["case_id"], record["email"])
        return record

    def list_by_email(self, email: str) -> list[dict[str, Any]]:
        needle = (email or "").strip().lower()
        if not needle:
            return []
        return [item for item in self._read() if str(item.get("email") or "").lower() == needle]
