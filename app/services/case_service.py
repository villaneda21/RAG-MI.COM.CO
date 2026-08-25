"""Registro persistente de casos de soporte (apertura y cierre)."""

from __future__ import annotations

import json
import logging
import re
import uuid
from dataclasses import asdict, dataclass
from datetime import datetime
from pathlib import Path
from zoneinfo import ZoneInfo

from app.config.settings import settings

logger = logging.getLogger(__name__)

CATEGORIES = (
    "Correo electrónico",
    "Dominio",
    "Hosting",
    "Facturación o Compras",
)

EMAIL_PATTERN = re.compile(r"^[^@\s]+@[^@\s]+\.[^@\s]+$")
BOGOTA = ZoneInfo("America/Bogota")


class CaseRegistrationError(Exception):
    def __init__(self, message: str, user_message: str | None = None) -> None:
        super().__init__(message)
        self.user_message = user_message or message


@dataclass
class CaseRecord:
    """Historial de un caso cerrado/registrado."""

    case_id: str
    correo: str
    categoria: str
    detalle: str
    registrado_en: str
    estado: str
    session_id: str = ""

    def to_dict(self) -> dict:
        return asdict(self)


class CaseService:
    """Guarda cada caso en un historial local (JSONL)."""

    def __init__(self, store_path: Path | None = None) -> None:
        self.store_path = Path(store_path or settings.cases_file)

    def register(
        self,
        correo: str,
        categoria: str,
        detalle: str,
        session_id: str = "",
    ) -> CaseRecord:
        email = (correo or "").strip().lower()
        notes = (detalle or "").strip()
        category = _normalize_category(categoria)

        if not EMAIL_PATTERN.match(email):
            raise CaseRegistrationError(
                f"Correo inválido: {correo}",
                user_message="El correo no parece válido. ¿Me lo confirmas de nuevo?",
            )
        if category not in CATEGORIES:
            raise CaseRegistrationError(
                f"Categoría inválida: {categoria}",
                user_message="¿El tema es correo, dominio, hosting o facturación/compras?",
            )
        if len(notes) < 3:
            raise CaseRegistrationError(
                "El detalle del caso está vacío.",
                user_message="Cuéntame un poco más del motivo de tu contacto para dejarlo registrado.",
            )

        now = datetime.now(BOGOTA)
        record = CaseRecord(
            case_id=_new_case_id(now),
            correo=email,
            categoria=category,
            detalle=notes,
            registrado_en=now.strftime("%Y-%m-%d %H:%M:%S %Z"),
            estado="Cerrado/Registrado",
            session_id=session_id or "",
        )
        self._append(record)
        logger.info("Caso %s registrado (%s, %s).", record.case_id, record.categoria, record.correo)
        return record

    def list_cases(self, limit: int = 50) -> list[CaseRecord]:
        if not self.store_path.exists():
            return []
        records: list[CaseRecord] = []
        for line in self.store_path.read_text(encoding="utf-8").splitlines():
            if not line.strip():
                continue
            try:
                payload = json.loads(line)
                records.append(CaseRecord(**payload))
            except (json.JSONDecodeError, TypeError):
                continue
        return list(reversed(records[-limit:]))

    def _append(self, record: CaseRecord) -> None:
        self.store_path.parent.mkdir(parents=True, exist_ok=True)
        with self.store_path.open("a", encoding="utf-8") as handle:
            handle.write(json.dumps(record.to_dict(), ensure_ascii=False) + "\n")


def _normalize_category(value: str) -> str:
    raw = (value or "").strip().lower()
    aliases = {
        "correo electrónico": "Correo electrónico",
        "correo electronico": "Correo electrónico",
        "correo": "Correo electrónico",
        "email": "Correo electrónico",
        "dominio": "Dominio",
        "hosting": "Hosting",
        "facturación o compras": "Facturación o Compras",
        "facturacion o compras": "Facturación o Compras",
        "facturación": "Facturación o Compras",
        "facturacion": "Facturación o Compras",
        "compras": "Facturación o Compras",
        "factura": "Facturación o Compras",
    }
    return aliases.get(raw, value.strip())


def _new_case_id(moment: datetime) -> str:
    suffix = uuid.uuid4().hex[:4].upper()
    return f"CASO-{moment.strftime('%Y%m%d-%H%M%S')}-{suffix}"
