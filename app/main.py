"""Aplicación FastAPI: interfaz del chatbot y API RAG."""

from __future__ import annotations

import logging
from contextlib import asynccontextmanager
from fastapi import FastAPI, HTTPException, Request
from fastapi.responses import HTMLResponse, JSONResponse
from fastapi.staticfiles import StaticFiles
from fastapi.templating import Jinja2Templates
from pydantic import ValidationError

from app.config.settings import PROJECT_ROOT, settings
from app.models.schemas import ChatRequest, HealthResponse, ReindexRequest, ReindexResponse
from app.services.claude_service import ClaudeService, ClaudeServiceError
from app.services.document_service import DocumentService
from app.services.rag_service import RagService
from app.services.vector_service import VectorService, VectorStoreError
from app.utils.text_processor import DocumentProcessingError

logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s | %(levelname)s | %(name)s | %(message)s",
)
logger = logging.getLogger("rag_claude")

templates = Jinja2Templates(directory=str(PROJECT_ROOT / "templates"))


def _build_rag_service() -> RagService:
    return RagService(
        document_service=DocumentService(),
        vector_service=VectorService(),
        claude_service=ClaudeService(),
    )


@asynccontextmanager
async def lifespan(app: FastAPI):
    logger.info("Inicio del sistema RAG MI.COM.CO")
    logger.info("Raíz del proyecto: %s", PROJECT_ROOT)
    logger.info("Modelo de embeddings: %s", settings.embedding_model)
    logger.info("Modelo de Claude: %s", settings.claude_model)
    logger.info("Claude configurado: %s", "sí" if settings.claude_is_configured else "no (faltará para /api/chat)")

    rag = _build_rag_service()
    app.state.rag = rag
    app.state.cases = rag.case_service
    try:
        rag.vector_service.connect()
        logger.info(
            "ChromaDB listo. Chunks indexados: %s",
            rag.vector_service.count(),
        )
    except VectorStoreError:
        logger.exception("ChromaDB no está disponible al iniciar. El health lo reportará.")
    yield
    logger.info("Sistema RAG detenido.")


app = FastAPI(
    title="MI.COM.CO — Asistente RAG",
    description=(
        "Sistema de Retrieval-Augmented Generation sobre la base de conocimiento "
        "de MI.COM.CO. Recupera fragmentos desde ChromaDB y genera respuestas con Claude."
    ),
    version="1.0.0",
    lifespan=lifespan,
)

static_dir = PROJECT_ROOT / "static"
static_dir.mkdir(parents=True, exist_ok=True)
app.mount("/static", StaticFiles(directory=str(static_dir)), name="static")


def get_rag(request: Request) -> RagService:
    rag = getattr(request.app.state, "rag", None)
    if rag is None:
        rag = _build_rag_service()
        request.app.state.rag = rag
    return rag


@app.exception_handler(DocumentProcessingError)
async def document_error_handler(_request: Request, exc: DocumentProcessingError) -> JSONResponse:
    logger.error("Error de documento: %s", exc)
    return JSONResponse(status_code=400, content={"detail": exc.user_message})


@app.exception_handler(VectorStoreError)
async def vector_error_handler(_request: Request, exc: VectorStoreError) -> JSONResponse:
    logger.error("Error de ChromaDB: %s", exc)
    return JSONResponse(status_code=503, content={"detail": exc.user_message})


@app.exception_handler(ClaudeServiceError)
async def claude_error_handler(_request: Request, exc: ClaudeServiceError) -> JSONResponse:
    logger.error("Error de Claude: %s", exc)
    return JSONResponse(status_code=503, content={"detail": exc.user_message})


@app.get("/", response_class=HTMLResponse)
async def home(request: Request) -> HTMLResponse:
    """Carga la interfaz web del chatbot."""
    return templates.TemplateResponse(
        "index.html",
        {"request": request, "assistant_name": "Asistente MI.COM.CO"},
    )


@app.get("/health", response_model=HealthResponse)
async def health(request: Request) -> HealthResponse:
    """Estado del sistema: API, ChromaDB y configuración de Claude."""
    rag = get_rag(request)
    vector_status = "error"
    indexed = 0
    overall = "degraded"

    try:
        if rag.vector_service.is_connected():
            vector_status = "connected"
            indexed = rag.vector_service.count()
            overall = "ok"
    except Exception:  # noqa: BLE001
        logger.exception("Health check de ChromaDB falló.")
        vector_status = "error"

    claude_status = "configured" if rag.claude_service.is_configured else "missing_api_key"
    return HealthResponse(
        status=overall,
        vector_database=vector_status,
        indexed_chunks=indexed,
        claude_api=claude_status,
    )


@app.post("/api/chat")
async def chat(payload: ChatRequest, request: Request):
    """Continúa el flujo de atención y registro de casos."""
    rag = get_rag(request)
    try:
        history = [{"role": item.role, "content": item.content} for item in payload.history]
        result = await rag.ask(
            payload.question,
            history=history,
            session_id=payload.session_id,
        )
    except ValueError as exc:
        raise HTTPException(status_code=400, detail=str(exc)) from exc
    except (VectorStoreError, ClaudeServiceError, DocumentProcessingError):
        raise
    except Exception:
        logger.exception("Error inesperado al procesar /api/chat.")
        raise HTTPException(
            status_code=500,
            detail="Ocurrió un error interno al generar la respuesta. Revisa la consola del servidor.",
        ) from None
    return result


@app.post("/api/reindex", response_model=ReindexResponse)
async def reindex(request: Request, payload: ReindexRequest | None = None) -> ReindexResponse:
    """Vuelve a procesar data/documents/documento.txt y actualiza ChromaDB."""
    options = payload or ReindexRequest()
    rag = get_rag(request)
    logger.info("Endpoint /api/reindex (reset=%s).", options.reset)
    return rag.reindex(reset=options.reset)


@app.get("/api/cases")
async def list_cases(request: Request, limit: int = 20):
    """Lista los casos más recientes del historial local."""
    rag = get_rag(request)
    records = rag.case_service.list_cases(limit=max(1, min(limit, 100)))
    return [record.to_dict() for record in records]


@app.exception_handler(ValidationError)
async def validation_error_handler(_request: Request, exc: ValidationError) -> JSONResponse:
    return JSONResponse(status_code=422, content={"detail": "La solicitud no es válida."})
