"""Pruebas del bootstrap de run.py (Python 3.12 y rutas del venv)."""

from __future__ import annotations

import os

import run as run_module
from app.config.settings import is_usable_anthropic_key


def test_requires_python_312() -> None:
    assert run_module.is_required_python((3, 12)) is True
    assert run_module.is_required_python((3, 11)) is False
    assert run_module.is_required_python((3, 13)) is False


def test_venv_python_path_matches_os() -> None:
    python_path = run_module.venv_python()
    assert python_path.parent.name in {"Scripts", "bin"}
    if os.name == "nt":
        assert python_path.name.lower() == "python.exe"
    else:
        assert python_path.name == "python"


def test_project_root_contains_requirements() -> None:
    assert run_module.REQUIREMENTS.exists()
    assert run_module.ROOT.name  # no vacío
    assert (run_module.ROOT / "run.py").exists()


def test_requirements_hash_is_stable() -> None:
    first = run_module.requirements_hash()
    second = run_module.requirements_hash()
    assert first == second
    assert len(first) == 64


def test_placeholder_api_key_is_rejected() -> None:
    assert is_usable_anthropic_key("") is False
    assert is_usable_anthropic_key("tu_api_key_aqui") is False
    assert is_usable_anthropic_key("sk-ant-api03-realish-value") is True
