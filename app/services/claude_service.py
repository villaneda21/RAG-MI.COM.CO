"""Único módulo que habla con la API oficial de Anthropic (Claude)."""

from __future__ import annotations

import logging

from app.config.settings import is_usable_anthropic_key, settings

logger = logging.getLogger(__name__)

_MISSING_MODEL_HINT = (
    "El modelo de Claude configurado ya no está disponible (Anthropic lo retiró). "
    "En tu archivo .env pon CLAUDE_MODEL=claude-haiku-4-5 y reinicia el servidor."
)


def models_to_try(preferred: str, fallbacks: tuple[str, ...] | list[str]) -> list[str]:
    """Modelo pedido primero, luego alternativas sin repetir."""
    ordered: list[str] = []
    for name in (preferred, *fallbacks):
        cleaned = (name or "").strip()
        if cleaned and cleaned not in ordered:
            ordered.append(cleaned)
    return ordered


def user_message_for_api_status(status_code: int | None, detail: str, model: str) -> str:
    """Traduce errores HTTP de Anthropic a un mensaje claro para el chatbot."""
    text = (detail or "").lower()
    code = status_code or 0

    if code in {404, 400} and ("model" in text or "not_found" in text or code == 404):
        return _MISSING_MODEL_HINT
    if code in {401, 403} and ("credit" in text or "billing" in text or "quota" in text or "plan" in text):
        return (
            "Tu cuenta de Anthropic no tiene crédito o permiso para usar Claude. "
            "Revisa facturación en https://console.anthropic.com/"
        )
    if code == 403:
        return (
            "Anthropic rechazó la consulta (permiso o facturación). "
            "Revisa tu plan en https://console.anthropic.com/"
        )
    if code == 429:
        return "El servicio de Claude está ocupado. Espera un momento e inténtalo de nuevo."
    if code == 529:
        return "Claude está saturado en este momento. Inténtalo de nuevo en unos segundos."
    if code >= 500:
        return "Claude no pudo procesar la consulta. Inténtalo de nuevo más tarde."
    return (
        "Claude no pudo procesar la consulta. "
        "Si el problema continúa, revisa CLAUDE_MODEL y tu crédito en la consola de Anthropic."
    )


def _error_detail(exc: BaseException) -> str:
    body = getattr(exc, "body", None)
    if isinstance(body, dict):
        error = body.get("error") or {}
        if isinstance(error, dict):
            return str(error.get("message") or error)
        return str(body)
    message = getattr(exc, "message", None)
    return str(message or exc)


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
        return is_usable_anthropic_key(self.api_key)

    def _require_key(self) -> None:
        if not is_usable_anthropic_key(self.api_key):
            raise ClaudeServiceError(
                "ANTHROPIC_API_KEY no está definida o sigue siendo un placeholder.",
                user_message=(
                    "Falta configurar la clave de Claude. "
                    "Edita el archivo .env, pon tu ANTHROPIC_API_KEY real y reinicia el servidor."
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
        candidates = models_to_try(self.model, settings.claude_fallback_models)
        logger.info("Consulta a Claude (preferido=%s).", self.model)

        message = None
        last_status_error = None
        try:
            for index, model_name in enumerate(candidates):
                try:
                    message = await client.messages.create(
                        model=model_name,
                        max_tokens=settings.claude_max_tokens,
                        temperature=settings.claude_temperature,
                        system=SYSTEM_PROMPT,
                        messages=[{"role": "user", "content": user_payload}],
                    )
                    if model_name != self.model:
                        logger.warning(
                            "El modelo %s no está disponible. Se usó %s.",
                            self.model,
                            model_name,
                        )
                    break
                except anthropic.APIStatusError as exc:
                    last_status_error = exc
                    detail = _error_detail(exc)
                    logger.warning(
                        "Claude HTTP %s con modelo %s: %s",
                        exc.status_code,
                        model_name,
                        detail,
                    )
                    can_fallback = index < len(candidates) - 1 and (
                        exc.status_code == 404
                        or (exc.status_code == 400 and "model" in detail.lower())
                    )
                    if can_fallback:
                        continue
                    raise ClaudeServiceError(
                        f"Claude API status {exc.status_code}: {detail}",
                        user_message=user_message_for_api_status(
                            exc.status_code, detail, model_name
                        ),
                    ) from exc
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
        except ClaudeServiceError:
            raise
        except anthropic.APIStatusError as exc:
            detail = _error_detail(exc)
            raise ClaudeServiceError(
                f"Claude API status {exc.status_code}: {detail}",
                user_message=user_message_for_api_status(exc.status_code, detail, self.model),
            ) from exc
        except Exception as exc:  # noqa: BLE001
            logger.exception("Error inesperado al llamar a Claude.")
            raise ClaudeServiceError(
                f"Error inesperado con Claude: {exc}",
                user_message="Ocurrió un problema al generar la respuesta. Inténtalo de nuevo.",
            ) from exc

        if message is None:
            detail = _error_detail(last_status_error) if last_status_error else ""
            raise ClaudeServiceError(
                "Claude no devolvió una respuesta.",
                user_message=user_message_for_api_status(
                    getattr(last_status_error, "status_code", None),
                    detail,
                    self.model,
                ),
            )

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
