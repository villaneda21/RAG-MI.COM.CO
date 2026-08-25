"""Punto de entrada local del servidor FastAPI."""

from __future__ import annotations

import uvicorn

from app.config.settings import settings


if __name__ == "__main__":
    uvicorn.run(
        "app.main:app",
        host=settings.host,
        port=settings.port,
        reload=True,
    )
