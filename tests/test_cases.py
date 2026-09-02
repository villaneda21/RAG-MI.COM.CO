"""Pruebas del registro de casos de soporte."""

from __future__ import annotations

from pathlib import Path

import pytest

from app.services.case_service import CaseRegistrationError, CaseService


def test_register_case_writes_history(tmp_path: Path) -> None:
    store = CaseService(store_path=tmp_path / "casos.jsonl")
    record = store.register(
        correo="cliente@mi.com.co",
        categoria="Hosting",
        detalle="No puedo entrar al cPanel desde ayer.",
        session_id="ses-test",
    )
    assert record.estado == "Cerrado/Registrado"
    assert record.correo == "cliente@mi.com.co"
    assert record.categoria == "Hosting"
    assert record.case_id.startswith("CASO-")
    listed = store.list_cases()
    assert len(listed) == 1
    assert listed[0].detalle.startswith("No puedo entrar")


def test_register_case_rejects_invalid_email(tmp_path: Path) -> None:
    store = CaseService(store_path=tmp_path / "casos.jsonl")
    with pytest.raises(CaseRegistrationError):
        store.register(correo="no-es-correo", categoria="Dominio", detalle="El dominio no abre.")


def test_register_case_normalizes_category(tmp_path: Path) -> None:
    store = CaseService(store_path=tmp_path / "casos.jsonl")
    record = store.register(
        correo="ana@empresa.co",
        categoria="correo",
        detalle="No me llegan los mensajes al buzón.",
    )
    assert record.categoria == "Correo electrónico"


def test_colombia_timezone_does_not_crash() -> None:
    from app.services.case_service import colombia_tz

    zone = colombia_tz()
    stamp = __import__("datetime").datetime.now(zone)
    assert stamp.tzinfo is not None
