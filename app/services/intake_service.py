"""Flujo de atención: captura de datos, orientación con la base y cierre del caso.

No diagnostica ni ajusta conexiones (DNS, SPF/DKIM/DMARC, ping, red o servidores).
Las respuestas de fondo siguen saliendo de la base de conocimiento vía RAG.
"""

from __future__ import annotations

import re
import unicodedata
from dataclasses import dataclass, field, replace
from typing import Iterable, Literal, Optional

from app.services.case_store import CASE_STATUS_CLOSED, CaseStore

CategoryId = Literal["correo", "dominio", "hosting", "facturacion"]
IntakeStep = Literal[
    "idle",
    "awaiting_email",
    "awaiting_reason",
    "helping",
    "awaiting_close",
    "closed",
]

CATEGORY_ORDER: tuple[CategoryId, ...] = ("correo", "dominio", "hosting", "facturacion")


@dataclass(frozen=True)
class SupportCategory:
    id: CategoryId
    label: str
    description: str
    keywords: tuple[str, ...]


CATEGORIES: dict[CategoryId, SupportCategory] = {
    "correo": SupportCategory(
        id="correo",
        label="Correo electrónico",
        description="Buzones, webmail y correo corporativo.",
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
            "spam",
            "mensajeria",
        ),
    ),
    "dominio": SupportCategory(
        id="dominio",
        label="Dominio",
        description="Registro, transferencia, privacidad y administración del dominio.",
        keywords=(
            "dominio",
            "dominios",
            "epp",
            "whois",
            "antirrobo",
            "transferencia de dominio",
            "extender dominio",
            "renovar dominio",
        ),
    ),
    "hosting": SupportCategory(
        id="hosting",
        label="Hosting",
        description="Hosting, cPanel, sitio web y panel de administración.",
        keywords=(
            "hosting",
            "cpanel",
            "sitio web",
            "pagina web",
            "micconstructor",
            "miconstructor",
            "constructor web",
            "ssl",
            "disco",
            "backup",
        ),
    ),
    "facturacion": SupportCategory(
        id="facturacion",
        label="Factura o Compra",
        description="Facturas, pagos, renovaciones, cotizaciones y compras.",
        keywords=(
            "factura",
            "facturas",
            "facturacion",
            "compra",
            "compras",
            "pago",
            "pagos",
            "pagar",
            "cobro",
            "renovacion",
            "renovar",
            "precio",
            "saldo",
            "recibo",
            "openpay",
            "tarjeta",
            "pse",
            "cotizacion",
            "cotizar",
            "contratar",
            "plan nuevo",
            "planes",
            "ventas",
            "adquirir",
        ),
    ),
}

_EMAIL_RE = re.compile(r"\b[A-Z0-9._%+\-]+@[A-Z0-9.\-]+\.[A-Z]{2,}\b", re.IGNORECASE)

_CASE_START_PATTERNS = (
    r"asesor(?:a|es)? humano",
    r"agente humano",
    r"persona real",
    r"atenci[oó]n humana",
    r"hablar con (?:un |una )?(?:asesor|asesora|humano|humana|persona|agente|operador)",
    r"comunic(?:ar|arme|arnos) con (?:un |una )?(?:asesor|asesora|humano|humana|persona|agente)",
    r"pas(?:a|ame|arme) con (?:un |una )?(?:asesor|asesora|humano|agente)",
    r"quiero (?:un |una |hablar con )?(?:asesor|asesora|humano|humana)",
    r"necesito (?:un |una )?(?:asesor|asesora|humano|humana)",
    r"abrir (?:un )?caso",
    r"registrar (?:un )?caso",
    r"dejar (?:un )?caso",
    r"hablar con soporte",
    r"atenci[oó]n al cliente",
)
_CASE_START_REGEXES = tuple(re.compile(pattern) for pattern in _CASE_START_PATTERNS)

_CONNECTION_PATTERNS = (
    r"\bdns\b",
    r"\bspf\b",
    r"\bdkim\b",
    r"\bdmarc\b",
    r"\bping\b",
    r"traceroute",
    r"nameserver",
    r"name server",
    r"nslookup",
    r"registro(?:s)? mx",
    r"registro a\b",
    r"\bcname\b",
    r"zona dns",
    r"txt record",
    r"registro txt",
    r"conectividad",
    r"estado (?:del |de )?(?:servidor|red|ip)",
    r"ip p[uú]blica",
    r"verificar (?:el )?mx",
    r"revisar (?:el |los )?(?:dns|spf|dkim|dmarc|ping)",
    r"apuntar (?:el )?dominio",
)
_CONNECTION_REGEXES = tuple(re.compile(pattern) for pattern in _CONNECTION_PATTERNS)

_CONFIRM_RE = re.compile(
    r"^(si|sí|ok|okay|dale|listo|correcto|confirmo|confirmado|exacto|asi es|así es|"
    r"esta bien|está bien|de acuerdo|registra(?:lo)?|cierra(?:lo)?|cerrarlo|"
    r"si,?\s*esta correcto|sí,?\s*está correcto|si registra|sí registra)\b"
)
_MORE_HELP_RE = re.compile(
    r"(otra duda|otra pregunta|aun no|aún no|no me (?:sirve|ayudo|ayudó)|"
    r"no se resolvio|no se resolvió|sigue (?:sin|igual)|mas ayuda|más ayuda)"
)
_DENY_RE = re.compile(r"^(no|nop|nel|espera|corregir|cambiar|mal|error)\b")

_CATEGORY_CHIP: dict[CategoryId, tuple[str, ...]] = {
    "correo": (r"area de correo", r"correo electronico", r"es de correo"),
    "dominio": (r"area de dominio", r"es de dominio"),
    "hosting": (r"area de hosting", r"es de hosting"),
    "facturacion": (r"area de factur", r"factura o compra", r"es de factura", r"es de compra"),
}


def normalize_text(value: str) -> str:
    decomposed = unicodedata.normalize("NFD", value or "")
    stripped = "".join(char for char in decomposed if unicodedata.category(char) != "Mn")
    return re.sub(r"\s+", " ", stripped.lower()).strip()


def extract_email(text: str) -> str | None:
    match = _EMAIL_RE.search(text or "")
    return match.group(0).lower() if match else None


def is_connection_restricted(text: str) -> bool:
    """True si piden diagnóstico, ajuste o verificación de conexiones."""
    normalized = normalize_text(text)
    return any(pattern.search(normalized) for pattern in _CONNECTION_REGEXES)


def wants_case_flow(
    question: str,
    requested_category: str | None = None,
    intake_active: bool = False,
) -> bool:
    if intake_active:
        return True
    if requested_category and requested_category in CATEGORIES:
        return True
    normalized = normalize_text(question)
    return any(pattern.search(normalized) for pattern in _CASE_START_REGEXES)


def category_from_id(category_id: str | None) -> SupportCategory | None:
    if not category_id:
        return None
    return CATEGORIES.get(category_id.strip().lower())  # type: ignore[arg-type]


def _contains_keyword(text: str, keyword: str) -> bool:
    if " " in keyword:
        return keyword in text
    if len(keyword) <= 3:
        return re.search(rf"\b{re.escape(keyword)}\b", text) is not None
    return keyword in text


def _score_category(normalized: str, category: SupportCategory) -> int:
    score = 0
    for keyword in category.keywords:
        if _contains_keyword(normalized, keyword):
            score += 2 if " " in keyword else 1
    return score


def _chip_category(normalized: str) -> CategoryId | None:
    for category_id, patterns in _CATEGORY_CHIP.items():
        if any(re.search(pattern, normalized) for pattern in patterns):
            return category_id
    return None


def classify_category(
    question: str,
    history: Iterable[str] | None = None,
    requested_category: str | None = None,
) -> CategoryId | None:
    explicit = category_from_id(requested_category)
    if explicit:
        return explicit.id

    current = normalize_text(question)
    chip = _chip_category(current)
    if chip:
        return chip

    history_text = normalize_text(" ".join(history or ()))
    current_scores = [(item, _score_category(current, CATEGORIES[item])) for item in CATEGORY_ORDER]
    history_scores = [(item, _score_category(history_text, CATEGORIES[item])) for item in CATEGORY_ORDER]
    ranked = sorted(
        (
            (item, current_score * 3 + history_score)
            for (item, current_score), (_, history_score) in zip(current_scores, history_scores)
        ),
        key=lambda pair: pair[1],
        reverse=True,
    )
    best_id, best_score = ranked[0]
    if best_score <= 0:
        return None
    if len(ranked) > 1 and ranked[1][1] == best_score:
        current_best = max(current_scores, key=lambda pair: pair[1])
        return current_best[0] if current_best[1] > 0 else None
    return best_id


def _looks_like_notes(question: str, email: str | None) -> bool:
    cleaned = (question or "").strip()
    if email:
        cleaned = re.sub(re.escape(email), "", cleaned, flags=re.IGNORECASE).strip()
    normalized = normalize_text(cleaned)
    if len(normalized) < 8:
        return False
    if _CONFIRM_RE.match(normalized) or _DENY_RE.match(normalized):
        return False
    if _chip_category(normalized) and len(normalized) < 40:
        return False
    if any(pattern.search(normalized) for pattern in _CASE_START_REGEXES) and len(normalized) < 40:
        return False
    return True


def _merge_notes(previous: str | None, incoming: str, email: str | None) -> str:
    cleaned = incoming.strip()
    if email:
        cleaned = re.sub(re.escape(email), "", cleaned, flags=re.IGNORECASE).strip(" ,.-")
    if not cleaned:
        return (previous or "").strip()
    if previous and cleaned.lower() in previous.lower():
        return previous
    if previous:
        return f"{previous.rstrip('. ')}. {cleaned}"
    return cleaned


def _pick(options: tuple[str, ...], seed: str) -> str:
    if not options:
        return ""
    return options[sum(ord(char) for char in seed) % len(options)]


def history_texts(
    history: Optional[Iterable[dict[str, str] | object]],
    roles: tuple[str, ...] = ("user",),
) -> list[str]:
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


@dataclass
class IntakeSnapshot:
    active: bool = False
    step: IntakeStep = "idle"
    email: str | None = None
    category: CategoryId | None = None
    notes: str | None = None
    summary: str | None = None
    case_id: str | None = None
    status: str | None = None
    registered_at: str | None = None
    registered_at_display: str | None = None
    blocked_connection: bool = False
    turn: int = 0

    @property
    def category_label(self) -> str | None:
        category = category_from_id(self.category)
        return category.label if category else None


@dataclass
class IntakeDecision:
    handled: bool
    skip_rag: bool
    wrap_after_rag: bool
    answer: str
    snapshot: IntakeSnapshot
    rag_question: str | None = None
    case: dict[str, str] | None = None
    extras: dict[str, bool] = field(default_factory=dict)


def _ask_email(snapshot: IntakeSnapshot) -> str:
    topic = snapshot.category_label
    if topic:
        return _pick(
            (
                f"¡Hola! Con gusto te ayudo con lo de {topic.lower()}. Para ubicar tu cuenta, ¿me compartes el correo con el que estás en mi.com.co?",
                f"Hola, claro que sí. Veo que es un tema de {topic.lower()}. ¿Me dejas el correo electrónico de tu cuenta para identificarla?",
            ),
            snapshot.notes or topic,
        )
    return _pick(
        (
            "¡Hola! Qué gusto saludarte. Para ubicar tu cuenta, ¿me compartes el correo con el que estás registrado en mi.com.co?",
            "Hola, con gusto te acompaño. ¿Me dejas el correo electrónico de tu cuenta para identificarla?",
            "Hola, sí te ayudo. Primero necesito el correo de tu cuenta, ¿me lo compartes?",
        ),
        snapshot.notes or "email",
    )


def _ask_reason(snapshot: IntakeSnapshot) -> str:
    email = snapshot.email or "tu cuenta"
    return _pick(
        (
            f"Perfecto, ya te tengo con {email}. Cuéntame con calma qué te está pasando, para orientarte con la base de conocimiento.",
            f"Listo, {email}. ¿El tema es de correo electrónico, dominio, hosting, o una factura o compra? Cuéntame el detalle y te ayudo.",
            f"Gracias. Con {email} ya podemos seguir. ¿Qué te trae por aquí hoy?",
        ),
        email,
    )


def _refuse_connection(snapshot: IntakeSnapshot) -> str:
    base = _pick(
        (
            "Eso lo dejo anotado, pero desde aquí no hago revisiones ni cambios de DNS, SPF, DKIM, DMARC, ping ni estado de red. El equipo lo toma desde el caso si hace falta.",
            "Te entiendo. En este chat no diagnosticamos ni ajustamos conexiones (DNS, registros de correo, IP o ping). Sí puedo orientarte con la base de conocimiento en lo demás y dejar el caso registrado.",
        ),
        snapshot.email or snapshot.notes or "dns",
    )
    if not snapshot.email:
        return f"{base} Mientras tanto, ¿me compartes el correo de tu cuenta?"
    if not snapshot.category:
        return f"{base} ¿Me confirmas si el caso es de correo, dominio, hosting, o factura/compra?"
    return f"{base} Si quieres, lo registramos en tu historial y lo dejamos cerrado para que quede constancia."


def _close_prompt(snapshot: IntakeSnapshot) -> str:
    return _pick(
        (
            "Si con esto ya quedamos, lo registro en el historial de tu cuenta y cerramos el caso. Si tienes otra duda, dime y seguimos.",
            "Cuando quieras, dejo el caso registrado y cerrado en tu historial. Si surgió algo más, aquí sigo.",
            "¿Te quedó más o menos claro? Si sí, lo guardo como cerrado en tu historial. Si no, seguimos.",
        ),
        snapshot.email or "cierre",
    )


def wrap_help_answer(kb_answer: str, snapshot: IntakeSnapshot) -> str:
    body = (kb_answer or "").strip()
    if snapshot.blocked_connection:
        return f"{_refuse_connection(snapshot)}"
    if not body:
        return _close_prompt(snapshot)
    return f"{body}\n\n{_close_prompt(snapshot)}"


def _goodbye(record: dict[str, str]) -> str:
    return (
        f"Listo, ya quedó registrado en el historial de tu cuenta con estado **Cerrado**. "
        f"Te identificamos con {record['email']}, el tipo de ayuda fue {record['help_type_label'].lower()} "
        f"y lo guardamos el {record['registered_at_display']}. "
        f"Gracias por escribirnos; aquí estaremos si más adelante surge otra cosa."
    )


def snapshot_flags(snapshot: IntakeSnapshot) -> dict[str, bool]:
    return {
        "needs_email": snapshot.active and snapshot.step == "awaiting_email",
        "needs_reason": snapshot.active and snapshot.step == "awaiting_reason",
        "needs_close": snapshot.active and snapshot.step == "awaiting_close",
        "show_categories": snapshot.active and snapshot.step == "awaiting_reason",
        "registered": snapshot.step == "closed",
        "blocked_connection": snapshot.blocked_connection,
    }


def _idle_decision() -> IntakeDecision:
    return IntakeDecision(
        handled=False,
        skip_rag=False,
        wrap_after_rag=False,
        answer="",
        snapshot=IntakeSnapshot(),
        extras={},
    )


def continue_intake(
    question: str,
    history: Iterable[str] | None = None,
    current: IntakeSnapshot | None = None,
    requested_category: str | None = None,
    store: CaseStore | None = None,
) -> IntakeDecision:
    """Avanza el flujo de atención. Si no aplica, handled=False para seguir con RAG normal."""
    incoming = (question or "").strip()
    snapshot = replace(current) if current and current.active else IntakeSnapshot()
    start = wants_case_flow(incoming, requested_category, snapshot.active)

    if not snapshot.active and not start:
        return _idle_decision()

    snapshot.active = True
    snapshot.turn += 1
    snapshot.blocked_connection = is_connection_restricted(incoming)

    found_email = extract_email(incoming)
    if found_email:
        snapshot.email = found_email

    classified = classify_category(
        incoming,
        history=history,
        requested_category=requested_category or snapshot.category,
    )
    if classified:
        snapshot.category = classified

    if _looks_like_notes(incoming, snapshot.email):
        snapshot.notes = _merge_notes(snapshot.notes, incoming, snapshot.email)

    normalized = normalize_text(incoming)
    wants_more = bool(_MORE_HELP_RE.search(normalized))
    confirms = bool(_CONFIRM_RE.match(normalized))
    denies = bool(_DENY_RE.match(normalized))

    if snapshot.step == "closed":
        snapshot = IntakeSnapshot(active=True, turn=1, email=snapshot.email)
        if found_email:
            snapshot.email = found_email
        if classified:
            snapshot.category = classified

    if snapshot.step in {"idle", "closed"}:
        snapshot.step = "awaiting_email"

    if denies and snapshot.step == "awaiting_close":
        snapshot.step = "awaiting_reason"
        snapshot.notes = None
        snapshot.category = requested_category or None if requested_category else snapshot.category
        return IntakeDecision(
            handled=True,
            skip_rag=True,
            wrap_after_rag=False,
            answer="Sin problema, lo ajustamos. Cuéntame de nuevo qué necesitas y lo dejamos bien encaminado.",
            snapshot=snapshot,
            extras=snapshot_flags(snapshot),
        )

    if wants_more and snapshot.step in {"awaiting_close", "helping"}:
        snapshot.step = "helping"
        snapshot.blocked_connection = is_connection_restricted(incoming)
        if snapshot.blocked_connection:
            snapshot.step = "awaiting_close"
            return IntakeDecision(
                handled=True,
                skip_rag=True,
                wrap_after_rag=False,
                answer=_refuse_connection(snapshot),
                snapshot=snapshot,
                extras=snapshot_flags(snapshot),
            )
        return IntakeDecision(
            handled=True,
            skip_rag=False,
            wrap_after_rag=True,
            answer="",
            snapshot=snapshot,
            rag_question=incoming,
            extras=snapshot_flags(snapshot),
        )

    if not snapshot.email:
        snapshot.step = "awaiting_email"
        answer = _refuse_connection(snapshot) if snapshot.blocked_connection else _ask_email(snapshot)
        return IntakeDecision(
            handled=True,
            skip_rag=True,
            wrap_after_rag=False,
            answer=answer,
            snapshot=snapshot,
            extras=snapshot_flags(snapshot),
        )

    if not snapshot.category or not snapshot.notes:
        if snapshot.blocked_connection and snapshot.category:
            snapshot.notes = _merge_notes(
                snapshot.notes,
                "Consulta sobre conexiones (DNS/red) que el chat no diagnostica ni ajusta.",
                snapshot.email,
            )
        if snapshot.category and snapshot.notes:
            pass
        else:
            snapshot.step = "awaiting_reason"
            answer = _refuse_connection(snapshot) if snapshot.blocked_connection else _ask_reason(snapshot)
            return IntakeDecision(
                handled=True,
                skip_rag=True,
                wrap_after_rag=False,
                answer=answer,
                snapshot=snapshot,
                extras=snapshot_flags(snapshot),
            )

    if confirms and snapshot.step == "awaiting_close" and snapshot.email and snapshot.category:
        return _register_case(snapshot, store)

    if snapshot.blocked_connection:
        snapshot.step = "awaiting_close"
        snapshot.notes = _merge_notes(
            snapshot.notes,
            "El cliente preguntó por un tema de conexiones; no se realizó diagnóstico ni ajuste.",
            snapshot.email,
        )
        return IntakeDecision(
            handled=True,
            skip_rag=True,
            wrap_after_rag=False,
            answer=_refuse_connection(snapshot),
            snapshot=snapshot,
            extras=snapshot_flags(snapshot),
        )

    rag_question = snapshot.notes or incoming
    snapshot.step = "helping"
    return IntakeDecision(
        handled=True,
        skip_rag=False,
        wrap_after_rag=True,
        answer="",
        snapshot=snapshot,
        rag_question=rag_question,
        extras=snapshot_flags(snapshot),
    )


def mark_ready_to_close(snapshot: IntakeSnapshot, kb_answer: str) -> IntakeSnapshot:
    updated = replace(snapshot, step="awaiting_close")
    summary_source = kb_answer.strip() if kb_answer.strip() else (snapshot.notes or "")
    updated.summary = summary_source[:600]
    return updated


def _register_case(snapshot: IntakeSnapshot, store: CaseStore | None) -> IntakeDecision:
    category = category_from_id(snapshot.category)
    summary = (snapshot.summary or snapshot.notes or "Atención registrada desde el chat.").strip()
    writer = store or CaseStore()
    record = writer.save(
        email=snapshot.email or "",
        help_type=category.id if category else "correo",
        help_type_label=category.label if category else "Correo electrónico",
        summary=summary,
        status=CASE_STATUS_CLOSED,
    )
    closed = replace(
        snapshot,
        active=False,
        step="closed",
        case_id=str(record["case_id"]),
        status=CASE_STATUS_CLOSED,
        registered_at=str(record["registered_at"]),
        registered_at_display=str(record["registered_at_display"]),
        summary=str(record["summary"]),
    )
    return IntakeDecision(
        handled=True,
        skip_rag=True,
        wrap_after_rag=False,
        answer=_goodbye(record),
        snapshot=closed,
        case={str(key): str(value) for key, value in record.items()},
        extras=snapshot_flags(closed),
    )


def snapshot_from_payload(payload: dict[str, object] | object | None) -> IntakeSnapshot | None:
    if payload is None:
        return None
    if isinstance(payload, IntakeSnapshot):
        return payload
    data = payload if isinstance(payload, dict) else getattr(payload, "model_dump", lambda: {})()
    if not data or not data.get("active"):
        step = str(data.get("step") or "idle") if data else "idle"
        if step not in {"awaiting_email", "awaiting_reason", "helping", "awaiting_close"}:
            return None
    category = data.get("category")
    category_id = category if category in CATEGORIES else None
    valid_steps = {"idle", "awaiting_email", "awaiting_reason", "helping", "awaiting_close", "closed"}
    raw_step = str(data.get("step") or "awaiting_email")
    step: IntakeStep = raw_step if raw_step in valid_steps else "awaiting_email"  # type: ignore[assignment]
    return IntakeSnapshot(
        active=bool(data.get("active")),
        step=step,
        email=str(data["email"]) if data.get("email") else None,
        category=category_id,  # type: ignore[arg-type]
        notes=str(data["notes"]) if data.get("notes") else None,
        summary=str(data["summary"]) if data.get("summary") else None,
        case_id=str(data["case_id"]) if data.get("case_id") else None,
        status=str(data["status"]) if data.get("status") else None,
        registered_at=str(data["registered_at"]) if data.get("registered_at") else None,
        registered_at_display=str(data["registered_at_display"]) if data.get("registered_at_display") else None,
        turn=int(data.get("turn") or 0),
    )
