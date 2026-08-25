"""Detección de pedido de asesor humano y ruteo al área correspondiente.

Cuando el cliente pide hablar con una persona, el chatbot no sigue el flujo RAG:
conecta la conversación y la asigna a Correo, HDR (dominio y hosting),
Facturación o Ventas según el tema.
"""

from __future__ import annotations

import re
import unicodedata
from dataclasses import dataclass
from typing import Iterable, Literal, Optional

AreaId = Literal["correo", "hdr", "facturacion", "ventas"]

AREA_ORDER: tuple[AreaId, ...] = ("correo", "hdr", "facturacion", "ventas")


@dataclass(frozen=True)
class SupportArea:
    id: AreaId
    label: str
    assigned_label: str
    description: str
    keywords: tuple[str, ...]


AREAS: dict[AreaId, SupportArea] = {
    "correo": SupportArea(
        id="correo",
        label="Correo",
        assigned_label="área de Correo",
        description="Buzones, webmail, registros MX y correo corporativo.",
        keywords=(
            "correo",
            "email",
            "e-mail",
            "webmail",
            "buzon",
            "buzones",
            "bandeja",
            "outlook",
            "thunderbird",
            "imap",
            "smtp",
            "pop3",
            "mx",
            "spam",
            "mensajeria",
        ),
    ),
    "hdr": SupportArea(
        id="hdr",
        label="HDR",
        assigned_label="área HDR (Dominio y Hosting)",
        description="Dominios, DNS, hosting, cPanel y sitios web.",
        keywords=(
            "hdr",
            "dominio",
            "dominios",
            "hosting",
            "cpanel",
            "dns",
            "epp",
            "ssl",
            "nameserver",
            "name server",
            "whois",
            "sitio web",
            "pagina web",
            "micconstructor",
            "miconstructor",
            "constructor web",
            "transferencia de dominio",
            "antirrobo",
            "registro a",
            "cname",
            "txt record",
        ),
    ),
    "facturacion": SupportArea(
        id="facturacion",
        label="Facturación",
        assigned_label="área de Facturación",
        description="Facturas, pagos, renovaciones y saldos.",
        keywords=(
            "factura",
            "facturas",
            "facturacion",
            "pago",
            "pagos",
            "pagar",
            "cobro",
            "cobros",
            "vencimiento",
            "vencida",
            "renovacion",
            "renovar",
            "precio",
            "precios",
            "saldo",
            "recibo",
            "openpay",
            "tarjeta",
            "pse",
            "deuda",
        ),
    ),
    "ventas": SupportArea(
        id="ventas",
        label="Ventas",
        assigned_label="área de Ventas",
        description="Planes nuevos, cotizaciones y contratación de servicios.",
        keywords=(
            "venta",
            "ventas",
            "comprar",
            "compra",
            "contratar",
            "contratacion",
            "cotizacion",
            "cotizar",
            "plan nuevo",
            "planes",
            "adquirir",
            "comercial",
            "promocion",
            "quiero un plan",
            "nuevo servicio",
        ),
    ),
}


_HANDOFF_PATTERNS = (
    r"asesor(?:a|es)? humano",
    r"agente humano",
    r"operador(?:a)? humano",
    r"persona real",
    r"atenci[oó]n humana",
    r"hablar con (?:un |una )?(?:asesor|asesora|humano|humana|persona|agente|operador|operadora)",
    r"comunic(?:ar|arme|arnos) con (?:un |una )?(?:asesor|asesora|humano|humana|persona|agente)",
    r"conectar(?:me)? con (?:un |una )?(?:asesor|asesora|humano|humana|persona|agente)",
    r"conect(?:a|ame|arme) con (?:un |una )?(?:asesor|asesora|humano|humana|agente)",
    r"pas(?:a|ame|arme) con (?:un |una )?(?:asesor|asesora|humano|humana|agente|operador)",
    r"quiero (?:un |una |hablar con )?(?:asesor|asesora|humano|humana)",
    r"necesito (?:un |una )?(?:asesor|asesora|humano|humana)",
    r"puedo hablar con (?:un |una )?(?:asesor|asesora|humano|humana|persona)",
    r"hay (?:alguien|una persona|un asesor)",
    r"atend(?:er|erme) (?:un |una )?(?:asesor|persona|humano)",
    r"no (?:me )?(?:sirve|ayuda) (?:el |este )?(?:bot|asistente|robot|chatbot)",
    r"prefiero (?:un |una )?(?:humano|humana|asesor|persona)",
    r"transfer(?:ir|eme|irme) (?:a |con )?(?:un |una )?(?:asesor|humano|persona)",
)

_ASSIGN_AREA_PATTERNS: dict[AreaId, tuple[str, ...]] = {
    "correo": (r"asign(?:ar|ame|arme) al? area de correo", r"area de correo"),
    "hdr": (r"asign(?:ar|ame|arme) al? area(?: de)? hdr", r"area hdr", r"dominio y hosting"),
    "facturacion": (r"asign(?:ar|ame|arme) al? area de factur", r"area de factur"),
    "ventas": (r"asign(?:ar|ame|arme) al? area de ventas", r"area de ventas"),
}

_HANDOFF_REGEXES = tuple(re.compile(pattern) for pattern in _HANDOFF_PATTERNS)
_ASSIGN_REGEXES = {
    area: tuple(re.compile(pattern) for pattern in patterns)
    for area, patterns in _ASSIGN_AREA_PATTERNS.items()
}

_AREA_WITH_ADVISOR = re.compile(
    r"(?:asesor|asesora|humano|humana|agente|operador|operadora|persona).{0,40}"
    r"(correo|email|webmail|hdr|hosting|dominio|factur|pago|ventas|comercial)"
    r"|"
    r"(correo|email|webmail|hdr|hosting|dominio|factur|pago|ventas|comercial)"
    r".{0,40}(?:asesor|asesora|humano|humana|agente)"
)


def normalize_text(value: str) -> str:
    """Minúsculas sin tildes para comparar frases en español."""
    decomposed = unicodedata.normalize("NFD", value or "")
    without_marks = "".join(char for char in decomposed if unicodedata.category(char) != "Mn")
    return re.sub(r"\s+", " ", without_marks.lower()).strip()


def area_from_id(area_id: str | None) -> SupportArea | None:
    if not area_id:
        return None
    return AREAS.get(area_id.strip().lower())  # type: ignore[arg-type]


def _mentions_handoff(normalized: str) -> bool:
    return any(pattern.search(normalized) for pattern in _HANDOFF_REGEXES)


def _explicit_area_from_assignment(normalized: str) -> AreaId | None:
    for area_id, patterns in _ASSIGN_REGEXES.items():
        if any(pattern.search(normalized) for pattern in patterns):
            return area_id
    return None


def _area_from_keyword_token(token: str) -> AreaId | None:
    if token in {"correo", "email", "webmail"}:
        return "correo"
    if token in {"hdr", "hosting", "dominio"}:
        return "hdr"
    if token.startswith("factur") or token == "pago":
        return "facturacion"
    if token in {"ventas", "comercial"}:
        return "ventas"
    return None


def _explicit_area_near_advisor(normalized: str) -> AreaId | None:
    match = _AREA_WITH_ADVISOR.search(normalized)
    if not match:
        return None
    token = next((group for group in match.groups() if group), "")
    return _area_from_keyword_token(token)


def _contains_keyword(text: str, keyword: str) -> bool:
    if " " in keyword:
        return keyword in text
    if len(keyword) <= 3:
        return re.search(rf"\b{re.escape(keyword)}\b", text) is not None
    return keyword in text


def _score_area(normalized: str, area: SupportArea) -> int:
    score = 0
    for keyword in area.keywords:
        if _contains_keyword(normalized, keyword):
            score += 2 if " " in keyword else 1
    return score


def classify_area(
    question: str,
    history: Iterable[str] | None = None,
    requested_area: str | None = None,
) -> AreaId | None:
    """Elige el área de soporte a partir del pedido, historial o selección explícita."""
    explicit = area_from_id(requested_area)
    if explicit:
        return explicit.id

    current = normalize_text(question)
    assigned = _explicit_area_from_assignment(current)
    if assigned:
        return assigned

    near_advisor = _explicit_area_near_advisor(current)
    if near_advisor:
        return near_advisor

    history_text = normalize_text(" ".join(history or ()))
    combined = f"{history_text} {current}".strip()
    scored = [(area_id, _score_area(combined, AREAS[area_id])) for area_id in AREA_ORDER]
    current_scored = [(area_id, _score_area(current, AREAS[area_id])) for area_id in AREA_ORDER]

    # El mensaje actual pesa más que el historial.
    ranked = sorted(
        (
            (area_id, current_score * 3 + history_score)
            for (area_id, history_score), (_, current_score) in zip(scored, current_scored)
        ),
        key=lambda item: item[1],
        reverse=True,
    )
    best_id, best_score = ranked[0]
    if best_score <= 0:
        return None
    runner_up = ranked[1][1] if len(ranked) > 1 else 0
    if best_score == runner_up:
        # Empate: prioriza el área mencionada en el último mensaje.
        current_best = max(current_scored, key=lambda item: item[1])
        if current_best[1] > 0:
            return current_best[0]
        return None
    return best_id


def wants_human_advisor(
    question: str,
    requested_area: str | None = None,
    handed_off_area: str | None = None,
) -> bool:
    """True si el usuario pide un asesor, elige un área o ya está transferido."""
    if handed_off_area:
        return True
    if requested_area and area_from_id(requested_area):
        return True
    normalized = normalize_text(question)
    if _explicit_area_from_assignment(normalized):
        return True
    return _mentions_handoff(normalized)


@dataclass(frozen=True)
class HandoffDecision:
    requested: bool
    connected: bool
    needs_area: bool
    queued_message: bool
    area: SupportArea | None
    answer: str

    @property
    def area_id(self) -> AreaId | None:
        return self.area.id if self.area else None


def _connecting_answer(area: SupportArea) -> str:
    return (
        f"## 🤝 Conectando con un asesor humano\n\n"
        f"Listo. Estoy haciendo la **conexión** con un especialista.\n\n"
        f"Tu conversación quedó **asignada al {area.assigned_label}**.\n\n"
        f"- Tema: {area.description}\n"
        f"- Un asesor de ese equipo te atenderá en breve.\n\n"
        f"Si quieres, deja un mensaje y se lo enviamos al área."
    )


def _ask_area_answer() -> str:
    return (
        "## 🤝 Conectar con un asesor humano\n\n"
        "Claro. Voy a **hacer la conexión** con un asesor humano.\n\n"
        "Indícame el área para asignar tu conversación:\n\n"
        "- **Correo** — buzones, webmail y correo corporativo\n"
        "- **HDR (Dominio y Hosting)** — dominios, DNS, hosting y cPanel\n"
        "- **Facturación** — facturas, pagos y renovaciones\n"
        "- **Ventas** — planes nuevos y cotizaciones\n"
    )


def _queued_answer(area: SupportArea) -> str:
    return (
        f"## 📨 Mensaje enviado\n\n"
        f"Tu mensaje se envió al **{area.assigned_label}**.\n\n"
        f"Un asesor lo revisará en breve. Si necesitas algo más, puedes seguir escribiendo "
        f"o volver al asistente virtual."
    )


def resolve_handoff(
    question: str,
    history: Iterable[str] | None = None,
    requested_area: str | None = None,
    handed_off_area: str | None = None,
) -> HandoffDecision:
    """Decide si hay que transferir, a qué área y qué responder al cliente."""
    already = area_from_id(handed_off_area)
    normalized = normalize_text(question)
    selecting_area = bool(area_from_id(requested_area) or _explicit_area_from_assignment(normalized))
    asking_human = _mentions_handoff(normalized)

    if already and not selecting_area and not asking_human:
        return HandoffDecision(
            requested=True,
            connected=True,
            needs_area=False,
            queued_message=True,
            area=already,
            answer=_queued_answer(already),
        )

    if not asking_human and not selecting_area:
        return HandoffDecision(
            requested=False,
            connected=False,
            needs_area=False,
            queued_message=False,
            area=None,
            answer="",
        )

    area_id = classify_area(
        question,
        history=history,
        requested_area=requested_area or (already.id if already else None),
    )
    area = AREAS[area_id] if area_id else None
    if area is None:
        return HandoffDecision(
            requested=True,
            connected=False,
            needs_area=True,
            queued_message=False,
            area=None,
            answer=_ask_area_answer(),
        )

    return HandoffDecision(
        requested=True,
        connected=True,
        needs_area=False,
        queued_message=False,
        area=area,
        answer=_connecting_answer(area),
    )


def history_texts(
    history: Optional[Iterable[dict[str, str] | object]],
    roles: tuple[str, ...] = ("user",),
) -> list[str]:
    """Extrae el texto de turnos previos (dict o modelo Pydantic)."""
    texts: list[str] = []
    for item in history or []:
        if isinstance(item, dict):
            role = str(item.get("role") or "user")
            content = str(item.get("content") or "")
        else:
            role = str(getattr(item, "role", "user") or "user")
            content = str(getattr(item, "content", "") or "")
        if roles and role not in roles:
            continue
        if content.strip():
            texts.append(content)
    return texts
