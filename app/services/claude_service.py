"""Único módulo que habla con la API oficial de Anthropic (Claude)."""

from __future__ import annotations

import json
import logging
from dataclasses import dataclass
from typing import Any

from app.config.settings import is_usable_anthropic_key, settings
from app.services.case_service import CaseRecord, CaseRegistrationError, CaseService, CATEGORIES

logger = logging.getLogger(__name__)

_MISSING_MODEL_HINT = (
    "El modelo de Claude configurado ya no está disponible (Anthropic lo retiró). "
    "En tu archivo .env pon CLAUDE_MODEL=claude-haiku-4-5 y reinicia el servidor."
)

REGISTER_CASE_TOOL = {
    "name": "registrar_caso",
    "description": (
        "Registra y cierra el caso en el historial del sistema. "
        "Úsalo SOLO cuando el cliente ya dio su correo, la categoría del servicio "
        "y el detalle de la solicitud, y confirmó que esos datos son correctos."
    ),
    "input_schema": {
        "type": "object",
        "properties": {
            "correo": {
                "type": "string",
                "description": "Correo electrónico del cliente.",
            },
            "categoria": {
                "type": "string",
                "enum": list(CATEGORIES),
                "description": "Categoría principal de la solicitud.",
            },
            "detalle": {
                "type": "string",
                "description": "Resumen claro del motivo de contacto.",
            },
        },
        "required": ["correo", "categoria", "detalle"],
    },
}


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


def _text_from_message(message: Any) -> str:
    parts: list[str] = []
    for block in getattr(message, "content", []) or []:
        text = getattr(block, "text", None)
        if text:
            parts.append(text)
    return "\n".join(parts).strip()


def _tool_use_from_message(message: Any) -> Any | None:
    for block in getattr(message, "content", []) or []:
        if getattr(block, "type", None) == "tool_use" and getattr(block, "name", "") == "registrar_caso":
            return block
    return None


SUPPORT_SYSTEM_PROMPT = """Eres un compañero de soporte de MI.COM.CO. Estás conversando en tiempo real con un cliente.

[REGLA DIRECTA E INVIOLABLE]
- Bajo ninguna circunstancia debes realizar cambios, ajustes, diagnósticos o verificaciones relacionadas con temas de conexiones (como DNS, registros SPF/DKIM/DMARC, conectividad IP, ping, o estado de red/servidores).
- Tu única función en este flujo es recopilar la información inicial para la apertura, gestión y cierre del caso.
- Si el cliente pide que revises DNS, SPF, ping, IPs o servidores, no lo hagas: reconoce el malestar con empatía y sigue el flujo de registro.

[TONO Y ESTILO]
- Comunícate de forma completamente natural, empática, fluida y amigable.
- Evita sonar como un contestador automático, un bot rígido o usar listas tipo checklist desalmadas.
- Utiliza un lenguaje cercano y profesional, como si fueras un compañero de soporte conversando en tiempo real.
- Varía las frases de saludo y transición para mantener la espontaneidad.
- Puedes usar uno o dos emojis con naturalidad, sin saturar.
- Español siempre. No menciones que eres Claude ni detalles internos del sistema.

[FLUJO DE ATENCIÓN PASO A PASO]
1. Saludo y captura de datos:
   - Saluda cálidamente y pide el correo electrónico del cliente para identificar su cuenta.
   - Indaga de forma cercana sobre el motivo de su contacto.
   - Clasifica la solicitud únicamente en una de estas categorías:
     * Correo electrónico
     * Dominio
     * Hosting
     * Facturación o Compras

2. Confirmación del requerimiento:
   - Cuando tengas correo, categoría y detalle, confirma brevemente en un tono conversacional que los datos son correctos.

3. Cierre y almacenamiento:
   - Cuando el cliente confirme, llama a la herramienta registrar_caso.
   - Después del registro, despídete cordialmente confirmando que la solicitud quedó registrada con éxito en su historial.
   - Incluye de forma breve el número de caso si el sistema te lo devolvió.

No registres el caso hasta que el cliente haya confirmado. No inventes un correo ni una categoría.
"""

RAG_SYSTEM_PROMPT = """Eres el asistente virtual de MI.COM.CO.
Respondes en español, de forma clara y profesional.
Utiliza únicamente la información del contexto recuperado. No inventes datos.
"""


class ClaudeServiceError(Exception):
    """Error al comunicarse con la API de Claude."""

    def __init__(self, message: str, user_message: str | None = None) -> None:
        super().__init__(message)
        self.user_message = user_message or (
            "No se pudo generar la respuesta en este momento. Inténtalo de nuevo."
        )


@dataclass
class SupportTurn:
    """Respuesta de un turno del flujo de atención."""

    text: str
    case: CaseRecord | None = None


class ClaudeService:
    """Cliente asíncrono de Anthropic."""

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
        """Respuesta puntual con contexto RAG (indexación / consultas técnicas)."""
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
        message = await self._create_message(
            messages=[{"role": "user", "content": user_payload}],
            system=RAG_SYSTEM_PROMPT,
            temperature=settings.claude_temperature,
        )
        answer = _text_from_message(message)
        if not answer:
            return "No encontré información suficiente sobre esta pregunta en la base de conocimiento disponible."
        return answer

    async def generate_support_reply(
        self,
        question: str,
        history: list[dict[str, str]] | None = None,
        session_id: str = "",
        case_service: CaseService | None = None,
    ) -> SupportTurn:
        """Conversación del flujo de atención y registro de casos."""
        messages = _history_to_messages(history or [], question)
        message = await self._create_message(
            messages=messages,
            system=SUPPORT_SYSTEM_PROMPT,
            tools=[REGISTER_CASE_TOOL],
            temperature=0.7,
        )

        tool_block = _tool_use_from_message(message)
        spoken = _text_from_message(message)
        if tool_block is None:
            return SupportTurn(text=spoken or "Cuéntame un poco más para dejarte registrado con calma.")

        store = case_service or CaseService()
        tool_input = getattr(tool_block, "input", {}) or {}
        try:
            record = store.register(
                correo=str(tool_input.get("correo") or ""),
                categoria=str(tool_input.get("categoria") or ""),
                detalle=str(tool_input.get("detalle") or ""),
                session_id=session_id,
            )
            tool_result = json.dumps(
                {
                    "ok": True,
                    "case_id": record.case_id,
                    "estado": record.estado,
                    "registrado_en": record.registrado_en,
                },
                ensure_ascii=False,
            )
        except CaseRegistrationError as exc:
            logger.warning("No se pudo registrar el caso: %s", exc)
            record = None
            tool_result = json.dumps({"ok": False, "error": exc.user_message}, ensure_ascii=False)

        follow_up = await self._create_message(
            messages=[
                *messages,
                {"role": "assistant", "content": message.content},
                {
                    "role": "user",
                    "content": [
                        {
                            "type": "tool_result",
                            "tool_use_id": tool_block.id,
                            "content": tool_result,
                        }
                    ],
                },
            ],
            system=SUPPORT_SYSTEM_PROMPT,
            tools=[REGISTER_CASE_TOOL],
            temperature=0.7,
        )
        farewell = _text_from_message(follow_up) or spoken
        if record:
            return SupportTurn(text=farewell, case=record)
        return SupportTurn(text=farewell or "¿Me confirmas el correo y el motivo para dejar el caso listo?")

    async def _create_message(
        self,
        *,
        messages: list[dict[str, Any]],
        system: str,
        tools: list[dict[str, Any]] | None = None,
        temperature: float | None = None,
    ) -> Any:
        self._require_key()
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
        payload: dict[str, Any] = {
            "max_tokens": settings.claude_max_tokens,
            "temperature": settings.claude_temperature if temperature is None else temperature,
            "system": system,
            "messages": messages,
        }
        if tools:
            payload["tools"] = tools

        try:
            for index, model_name in enumerate(candidates):
                try:
                    message = await client.messages.create(model=model_name, **payload)
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
        logger.info("Claude respondió.")
        return message


def _history_to_messages(history: list[dict[str, str]], question: str) -> list[dict[str, Any]]:
    messages: list[dict[str, Any]] = []
    for item in history:
        role = (item.get("role") or "").strip()
        content = (item.get("content") or "").strip()
        if role not in {"user", "assistant"} or not content:
            continue
        messages.append({"role": role, "content": content})
    messages.append({"role": "user", "content": question.strip()})
    return messages
