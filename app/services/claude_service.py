"""Único módulo que habla con la API oficial de Anthropic (Claude)."""

from __future__ import annotations

import logging

from app.config.settings import settings

logger = logging.getLogger(__name__)

SYSTEM_PROMPT = """Eres el asistente virtual de MI.COM.CO (CENTRAL COMERCIALIZADORA DE INTERNET S.A.S.).
Respondes siempre en español, de forma útil, clara y relativamente concisa.
Si el usuario pide una explicación detallada, puedes extenderte.

Reglas estrictas:
- Utiliza ÚNICAMENTE la información entregada en el contexto de la base de conocimiento.
- No inventes datos, políticas, precios, procedimientos ni contactos.
- Si la respuesta no se puede obtener del contexto, dilo con claridad, por ejemplo:
  "No encontré información suficiente sobre esta pregunta en la base de conocimiento disponible."
- No menciones que eres Claude ni detalles internos del sistema, salvo que te lo pidan.
- Cuando cites procedimientos, enumera los pasos con claridad.
"""


class ClaudeServiceError(Exception):
    """Error al comunicarse con la API de Claude."""

    def __init__(self, message: str, user_message: str | None = None) -> None:
        super().__init__(message)
        self.user_message = user_message or (
            "No se pudo generar la respuesta en este momento. Inténtalo de nuevo."
        )


class ClaudeService:
    """Cliente asíncrono de Anthropic con prompt de sistema para RAG."""

    def __init__(self, api_key: str | None = None, model: str | None = None) -> None:
        self.api_key = (api_key if api_key is not None else settings.anthropic_api_key).strip()
        self.model = model or settings.claude_model

    @property
    def is_configured(self) -> bool:
        return bool(self.api_key)

    def _require_key(self) -> None:
        if not self.api_key:
            raise ClaudeServiceError(
                "ANTHROPIC_API_KEY no está definida.",
                user_message=(
                    "Falta configurar la clave de Claude. "
                    "Crea un archivo .env con ANTHROPIC_API_KEY y reinicia el servidor."
                ),
            )

    async def generate_response(self, question: str, context: str) -> str:
        """Envía el contexto recuperado y la pregunta del usuario a Claude."""
        self._require_key()

        user_payload = (
            "INFORMACIÓN DE LA BASE DE CONOCIMIENTO:\n"
            f"{context.strip()}\n\n"
            "PREGUNTA DEL USUARIO:\n"
            f"{question.strip()}\n\n"
            "INSTRUCCIONES:\n"
            "Responde únicamente utilizando la información proporcionada.\n"
            "No inventes información.\n"
            "Si la respuesta no puede obtenerse del contexto, indícalo claramente."
        )

        try:
            import anthropic
        except ImportError as exc:  # pragma: no cover
            raise ClaudeServiceError(
                f"El paquete anthropic no está instalado: {exc}",
                user_message="Falta la librería de Anthropic. Ejecuta pip install -r requirements.txt.",
            ) from exc

        client = anthropic.AsyncAnthropic(api_key=self.api_key)
        logger.info("Consulta a Claude (%s).", self.model)

        try:
            message = await client.messages.create(
                model=self.model,
                max_tokens=settings.claude_max_tokens,
                temperature=settings.claude_temperature,
                system=SYSTEM_PROMPT,
                messages=[{"role": "user", "content": user_payload}],
            )
        except anthropic.AuthenticationError as exc:
            logger.exception("Error de autenticación con Claude. No se registra la API key.")
            raise ClaudeServiceError(
                "Anthropic rechazó la autenticación.",
                user_message="La clave de Claude no es válida. Revisa ANTHROPIC_API_KEY en tu archivo .env.",
            ) from exc
        except anthropic.RateLimitError as exc:
            logger.exception("Límite de tasa de Claude alcanzado.")
            raise ClaudeServiceError(
                "Rate limit de Anthropic.",
                user_message="El servicio de Claude está ocupado. Espera un momento e inténtalo de nuevo.",
            ) from exc
        except anthropic.APIConnectionError as exc:
            logger.exception("Error de conexión con Claude.")
            raise ClaudeServiceError(
                f"No se pudo conectar con la API de Claude: {exc}",
                user_message="No hay conexión con el servicio de Claude. Verifica tu red e inténtalo de nuevo.",
            ) from exc
        except anthropic.APIStatusError as exc:
            logger.exception("Error HTTP de la API de Claude: %s", exc.status_code)
            raise ClaudeServiceError(
                f"Claude API status {exc.status_code}: {exc}",
                user_message="Claude no pudo procesar la consulta. Inténtalo de nuevo más tarde.",
            ) from exc
        except Exception as exc:  # noqa: BLE001
            logger.exception("Error inesperado al llamar a Claude.")
            raise ClaudeServiceError(
                f"Error inesperado con Claude: {exc}",
                user_message="Ocurrió un problema al generar la respuesta. Inténtalo de nuevo.",
            ) from exc

        parts = []
        for block in message.content:
            text = getattr(block, "text", None)
            if text:
                parts.append(text)

        answer = "\n".join(parts).strip()
        if not answer:
            logger.warning("Claude devolvió una respuesta vacía.")
            return "No encontré información suficiente sobre esta pregunta en la base de conocimiento disponible."

        logger.info("Claude respondió (%s caracteres).", len(answer))
        return answer
