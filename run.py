"""Arranque del proyecto: crea el entorno Python 3.12 y levanta el servidor.

Uso (Windows / Linux / macOS):

    python run.py
    py -3.12 run.py

La primera ejecución crea `venv/`, instala dependencias, indexa el TXT
si hace falta y abre http://127.0.0.1:8000
"""

from __future__ import annotations

import argparse
import hashlib
import os
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent
VENV_DIR = ROOT / "venv"
REQUIREMENTS = ROOT / "requirements.txt"
REQUIRED_MAJOR = 3
REQUIRED_MINOR = 12
DEPS_MARKER = VENV_DIR / ".deps-ok"


def configure_stdio() -> None:
    """Evita crashes por tildes en consolas Windows (cp1252)."""
    os.environ.setdefault("PYTHONIOENCODING", "utf-8")
    os.environ.setdefault("PYTHONUTF8", "1")
    for stream in (sys.stdout, sys.stderr):
        reconfigure = getattr(stream, "reconfigure", None)
        if reconfigure is None:
            continue
        try:
            reconfigure(encoding="utf-8", errors="replace")
        except Exception:
            pass


def log(message: str) -> None:
    print(f"[run] {message}", flush=True)


def is_required_python(info: tuple[int, int] | None = None) -> bool:
    version = info or (sys.version_info.major, sys.version_info.minor)
    return version == (REQUIRED_MAJOR, REQUIRED_MINOR)


def venv_python() -> Path:
    if os.name == "nt":
        return VENV_DIR / "Scripts" / "python.exe"
    return VENV_DIR / "bin" / "python"


def running_inside_project_venv() -> bool:
    try:
        prefix = Path(sys.prefix).resolve()
        return prefix == VENV_DIR.resolve()
    except OSError:
        return False


def _python_version(executable: str) -> tuple[int, int] | None:
    try:
        output = subprocess.check_output(
            [executable, "-c", "import sys; print(f'{sys.version_info.major}.{sys.version_info.minor}')"],
            text=True,
            stderr=subprocess.DEVNULL,
        ).strip()
        major_s, minor_s = output.split(".")
        return int(major_s), int(minor_s)
    except (OSError, subprocess.CalledProcessError, ValueError):
        return None


def find_python_312() -> str:
    """Localiza un intérprete 3.12 usando el launcher de Windows si existe."""
    if is_required_python():
        return sys.executable

    candidates: list[list[str]] = []
    if os.name == "nt":
        candidates.append(["py", "-3.12"])
        candidates.append(["py", f"-{REQUIRED_MAJOR}.{REQUIRED_MINOR}"])
    candidates.extend(
        [
            ["python3.12"],
            ["python3"],
            ["python"],
        ]
    )

    for command in candidates:
        try:
            output = subprocess.check_output(
                [*command, "-c", "import sys; print(sys.executable); print(f'{sys.version_info.major}.{sys.version_info.minor}')"],
                text=True,
                stderr=subprocess.DEVNULL,
            )
        except (OSError, subprocess.CalledProcessError):
            continue
        lines = [line.strip() for line in output.splitlines() if line.strip()]
        if len(lines) < 2:
            continue
        executable, version = lines[0], lines[1]
        try:
            major_s, minor_s = version.split(".")
            if (int(major_s), int(minor_s)) == (REQUIRED_MAJOR, REQUIRED_MINOR):
                return executable
        except ValueError:
            continue

    raise SystemExit(
        "Se necesita Python 3.12 para este proyecto.\n"
        "Instálalo desde https://www.python.org/downloads/release/python-31210/\n"
        "En Windows, durante la instalación marca 'Add python.exe to PATH'\n"
        "y vuelve a ejecutar:  py -3.12 run.py"
    )


def relaunch_with(python: Path, extra_args: list[str]) -> None:
    command = [str(python), str(ROOT / "run.py"), *extra_args]
    log(f"Reiniciando con el entorno del proyecto: {python}")
    raise SystemExit(subprocess.call(command))


def create_venv(python_312: str) -> None:
    VENV_DIR.mkdir(parents=True, exist_ok=True)
    log(f"Creando entorno virtual Python 3.12 en {VENV_DIR} ...")
    subprocess.check_call([python_312, "-m", "venv", str(VENV_DIR)])
    if not venv_python().exists():
        raise SystemExit(f"No se pudo crear el entorno virtual en {VENV_DIR}")


def requirements_hash() -> str:
    return hashlib.sha256(REQUIREMENTS.read_bytes()).hexdigest()


def deps_are_installed() -> bool:
    marker = DEPS_MARKER
    if not marker.exists() or marker.read_text(encoding="utf-8").strip() != requirements_hash():
        return False
    probe = (
        "import fastapi, uvicorn, chromadb, sentence_transformers, anthropic, dotenv, jinja2"
    )
    result = subprocess.run(
        [str(venv_python()), "-c", probe],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        check=False,
    )
    return result.returncode == 0


def install_dependencies() -> None:
    python = str(venv_python())
    log("Instalando dependencias (la primera vez puede tardar varios minutos)...")
    subprocess.check_call([python, "-m", "pip", "install", "--upgrade", "pip"])
    if os.name == "nt":
        # En Windows el wheel CPU evita el torch CUDA, que suele romper el arranque.
        log("Instalando PyTorch CPU para Windows...")
        subprocess.check_call(
            [
                python,
                "-m",
                "pip",
                "install",
                "torch",
                "--index-url",
                "https://download.pytorch.org/whl/cpu",
            ]
        )
    subprocess.check_call([python, "-m", "pip", "install", "-r", str(REQUIREMENTS)])
    DEPS_MARKER.write_text(requirements_hash(), encoding="utf-8")
    log("Dependencias instaladas.")


def ensure_env_file() -> None:
    env_path = ROOT / ".env"
    example = ROOT / ".env.example"
    if env_path.exists() or not example.exists():
        return
    env_path.write_text(example.read_text(encoding="utf-8"), encoding="utf-8")
    log("Se creó .env desde .env.example. Agrega ANTHROPIC_API_KEY para chatear con Claude.")


def ensure_runtime(skip_setup: bool) -> None:
    """Crea venv 3.12, instala paquetes y se relanza dentro de ese entorno."""
    os.chdir(ROOT)
    if skip_setup:
        if not is_required_python():
            raise SystemExit(
                f"Este proyecto requiere Python 3.12 y ahora corre {sys.version.split()[0]}."
            )
        return

    python_312 = find_python_312()
    if not venv_python().exists():
        create_venv(python_312)

    venv_version = _python_version(str(venv_python()))
    if not is_required_python(venv_version):
        log("El venv no es Python 3.12; se recreará.")
        create_venv(python_312)

    if not deps_are_installed():
        install_dependencies()

    if not running_inside_project_venv():
        extra = [arg for arg in sys.argv[1:] if arg != "--skip-setup"]
        relaunch_with(venv_python(), extra)


def apply_runtime_env() -> None:
    os.environ.setdefault("ANONYMIZED_TELEMETRY", "False")
    os.environ.setdefault("CHROMA_TELEMETRY_IMPL", "none")
    os.environ.setdefault("TOKENIZERS_PARALLELISM", "false")
    os.environ.setdefault("HF_HUB_DISABLE_SYMLINKS_WARNING", "1")
    os.environ.setdefault("OMP_NUM_THREADS", "1")
    cache_dir = ROOT / ".cache" / "huggingface"
    cache_dir.mkdir(parents=True, exist_ok=True)
    os.environ.setdefault("HF_HOME", str(cache_dir))
    os.environ["RAG_BOOTSTRAPPED"] = "1"


def ensure_index_and_preload() -> None:
    from app.services.rag_service import RagService
    from app.services.vector_service import VectorService

    vector_service = VectorService()
    vector_service.connect()
    rag = RagService(vector_service=vector_service)
    status = rag.index_status()
    if vector_service.count() == 0 or status.pending_changes:
        log("Indexando archivos nuevos o modificados en data/documents/ ...")
        result = rag.reindex(reset=vector_service.count() == 0)
        log(result.message)
    else:
        log(f"ChromaDB ya tiene {vector_service.count()} fragmentos al día.")
    log("Precargando el modelo de embeddings...")
    vector_service.get_collection()
    log("Modelo de embeddings listo.")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Crea el entorno 3.12 y arranca el asistente RAG.")
    parser.add_argument("--skip-setup", action="store_true", help="No crear venv ni instalar paquetes.")
    parser.add_argument("--reload", action="store_true", help="Activar recarga de desarrollo (no recomendado en Windows).")
    parser.add_argument("--no-ingest", action="store_true", help="No indexar ni precargar embeddings al arrancar.")
    parser.add_argument("--host", default=os.getenv("HOST", "127.0.0.1"))
    parser.add_argument("--port", type=int, default=int(os.getenv("PORT", "8000")))
    return parser.parse_args()


def main() -> None:
    configure_stdio()
    args = parse_args()
    ensure_runtime(skip_setup=args.skip_setup)
    apply_runtime_env()
    ensure_env_file()

    if not args.no_ingest:
        try:
            ensure_index_and_preload()
        except Exception as exc:  # noqa: BLE001
            log(f"No se pudo preparar la base vectorial: {exc}")
            log("El servidor arrancará igual; usa /api/reindex o scripts/ingest_document.py")

    import uvicorn

    log(f"Servidor listo en http://127.0.0.1:{args.port}")
    uvicorn.run(
        "app.main:app",
        host=args.host,
        port=args.port,
        reload=args.reload,
        workers=1,
        log_level="info",
    )


if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        raise SystemExit(0)
