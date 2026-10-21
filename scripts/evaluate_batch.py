#!/usr/bin/env python3
"""Evaluación offline S14/S15 sobre archivos ticket_archivado_*.txt.

Lee la ráfaga inicial del cliente, descarta por completo a SOPORTE y la envía
a POST /api/client/triage y, en el seguimiento, POST /api/client/message.
La palabra CONFIRMO del archivo no entra en el problema: se envía aparte para
auditar support_cases.
"""

from __future__ import annotations

import argparse
import csv
import json
import re
import sys
import time
import unicodedata
import urllib.error
import urllib.request
from pathlib import Path

GUIDE_RE = re.compile(
    r"\b((?:GUA|LEG|DOM|HST|CFG|DAT|PRV|ANX|INTRO)-\d{3})\b",
    re.IGNORECASE,
)
HEADER_RE = re.compile(
    r"^\[(?P<stamp>[^\]]+)\]\s+(?P<mark>🎧|👤)\s+(?P<role>SOPORTE|CLIENTE)\s+\((?P<label>[^)]+)\)\s*$"
)
CHAT_RE = re.compile(r"^💬\s?(?P<text>.*)$")
EXPECT_RE = re.compile(r"^EXPECT:\s*(?P<value>\S+)\s*$", re.IGNORECASE)
DELIM_RE = re.compile(r"^-{10,}\s*$")
CONFIRMATIONS = {"confirmo", "si confirmo", "confirmo gracias", "confirmo el caso"}
ESCALATED_STATUSES = {"esperando_agente", "en_proceso"}
TRANSFER_MARKERS = (
    "transferido",
    "agente humano",
    "alta demanda",
    "no encontré información exacta",
    "no encontre informacion exacta",
    "un agente te atenderá",
    "un agente te atendera",
)

SAMPLE_CHECKS = {
    "ticket_archivado_01.txt": ("crear una cuenta", "verificación de correo"),
    "ticket_archivado_02.txt": ("código de cliente",),
    "ticket_archivado_03.txt": ("cpanel",),
    "ticket_archivado_04.txt": ("certificado ssl",),
    "ticket_archivado_05.txt": ("comunícame con soporte",),
}


def fold(text: str) -> str:
    normalized = unicodedata.normalize("NFD", text)
    stripped = "".join(ch for ch in normalized if unicodedata.category(ch) != "Mn")
    return stripped.lower()


def is_confirmation(text: str) -> bool:
    compact = " ".join(fold(text).replace(",", " ").split())
    return compact in CONFIRMATIONS


def parse_ticket(path: Path) -> dict:
    mode = None
    burst_open = False
    burst_closed = False
    parts: list[str] = []
    client_name = ""
    saw_confirmo = False
    expected = ""
    support_lines = 0
    client_lines = 0
    for raw in path.read_text(encoding="utf-8").splitlines():
        line = raw.strip()
        if not line or DELIM_RE.match(line):
            continue
        expect = EXPECT_RE.match(line)
        if expect:
            expected = expect.group("value").strip().upper()
            continue
        header = HEADER_RE.match(line)
        if header:
            role = header.group("role")
            if role == "SOPORTE":
                support_lines += 1
                if burst_open:
                    burst_closed = True
                mode = "support"
            else:
                if not client_name:
                    client_name = header.group("label").strip()
                mode = "client" if not burst_closed else "later"
            continue
        chat = CHAT_RE.match(line)
        if chat is None or mode == "support":
            continue
        text = chat.group("text").strip()
        if not text:
            continue
        client_lines += 1
        if is_confirmation(text):
            saw_confirmo = True
            continue
        if mode == "client":
            burst_open = True
            parts.append(text)
    problem = " ".join(parts).strip()
    return {
        "path": str(path),
        "file": path.name,
        "client_name": client_name or "Cliente",
        "problem": problem,
        "expected": expected,
        "saw_confirmo": saw_confirmo,
        "support_turns": support_lines,
        "client_lines": client_lines,
    }


def discover(root: Path) -> list[dict]:
    paths = sorted(root.rglob("ticket_archivado_*.txt"))
    return [parse_ticket(path) for path in paths]


def self_check(tickets: list[dict]) -> list[str]:
    errors: list[str] = []
    if len(tickets) < 5:
        errors.append(f"se esperaban al menos 5 archivos y hay {len(tickets)}")
    for ticket in tickets:
        problem = ticket["problem"]
        folded = fold(problem)
        if not problem:
            errors.append(f"{ticket['file']}: el problema del cliente quedó vacío")
        if "CANARIO-AGENTE" in problem or "🎧" in problem or "SOPORTE (" in problem:
            errors.append(f"{ticket['file']}: se coló texto de SOPORTE")
        if "confirmo" in folded:
            errors.append(f"{ticket['file']}: CONFIRMO entró en la falla inicial")
        if ticket["support_turns"] < 1:
            errors.append(f"{ticket['file']}: no había turnos de SOPORTE para descartar")
        if not ticket["client_name"]:
            errors.append(f"{ticket['file']}: falta el nombre del cliente")
        if ticket["expected"] not in {"ESCALATE"} and not GUIDE_RE.fullmatch(ticket["expected"]):
            errors.append(f"{ticket['file']}: EXPECT inválido ({ticket['expected']})")
        required = SAMPLE_CHECKS.get(ticket["file"])
        if required:
            for snippet in required:
                if snippet not in folded and snippet not in problem.lower():
                    errors.append(f"{ticket['file']}: no aparece «{snippet}»")
        if ticket["file"] == "ticket_archivado_04.txt" and "cpanel" in folded:
            errors.append("ticket_archivado_04.txt: el ruido de cPanel de SOPORTE entró al problema")
        if ticket["file"] == "ticket_archivado_01.txt" and "certificado ssl" in folded:
            errors.append("ticket_archivado_01.txt: el ruido SSL de SOPORTE entró al problema")
    return errors


def guide_in_messages(messages: list[dict]) -> str:
    for message in messages:
        if message.get("sender_type") not in {"bot", "system"}:
            continue
        match = GUIDE_RE.search(str(message.get("content") or ""))
        if match:
            return match.group(1).upper()
    return ""


def is_escalated(ticket: dict, messages: list[dict]) -> bool:
    status = str(ticket.get("status") or "")
    if status in ESCALATED_STATUSES:
        return True
    blob = fold(
        "\n".join(
            str(message.get("content") or "")
            for message in messages
            if message.get("sender_type") in {"bot", "system"}
        )
    )
    return any(fold(marker) in blob for marker in TRANSFER_MARKERS)


def classify(expected: str, outcome: str, guide: str) -> str:
    if expected == "ESCALATE":
        if outcome == "deflected":
            return "fp"
        if outcome == "escalated":
            return "tn"
        return "unresolved"
    if outcome == "deflected" and guide == expected:
        return "tp"
    if outcome == "deflected":
        return "fp"
    if outcome == "escalated":
        return "fn"
    return "unresolved"


def post_json(url: str, payload: dict, timeout: float) -> tuple[float, dict]:
    data = json.dumps(payload).encode("utf-8")
    request = urllib.request.Request(
        url,
        data=data,
        headers={
            "Content-Type": "application/json",
            "X-Eval-Run": "s14",
        },
        method="POST",
    )
    started = time.perf_counter()
    with urllib.request.urlopen(request, timeout=timeout) as response:
        body = json.loads(response.read().decode("utf-8"))
    elapsed = (time.perf_counter() - started) * 1000.0
    return elapsed, body


def slug(text: str, fallback: str) -> str:
    folded = fold(text)
    cleaned = "".join(ch if ch.isalnum() else "-" for ch in folded)
    compact = "-".join(part for part in cleaned.split("-") if part)
    return compact or fallback


def evaluate_live(tickets: list[dict], base_url: str, timeout: float) -> dict:
    rows = []
    for index, ticket in enumerate(tickets, start=1):
        identity = slug(ticket["client_name"], f"cliente-{index}")
        payload = {
            "name": ticket["client_name"],
            "domain": f"{identity}.test",
            "email_micomco": f"eval-{index:02d}@mi.com.co",
            "reason": ticket["problem"],
        }
        row = {
            "file": ticket["file"],
            "client_name": ticket["client_name"],
            "problem": ticket["problem"],
            "expected": ticket["expected"],
            "saw_confirmo": ticket["saw_confirmo"],
        }
        try:
            http_ms, body = post_json(
                base_url.rstrip("/") + "/api/client/triage", payload, timeout
            )
        except (urllib.error.URLError, TimeoutError, json.JSONDecodeError) as err:
            row.update(
                {
                    "error": str(err),
                    "outcome": "error",
                    "label": "error",
                    "http_ms": None,
                }
            )
            rows.append(row)
            continue
        messages = body.get("messages") or []
        ticket_body = body.get("ticket") or {}
        guide = guide_in_messages(messages)
        escalated = is_escalated(ticket_body, messages)
        if escalated:
            outcome = "escalated"
        elif guide:
            outcome = "deflected"
        else:
            outcome = "unresolved"
        evaluation = body.get("eval") or {}
        top_chunk = str(evaluation.get("top_chunk_id") or "")
        weighted = str(evaluation.get("weighted_chunk") or "")
        flat = str(evaluation.get("flat_chunk") or "")
        row.update(
            {
                "http_ms": round(http_ms, 3),
                "server_ms": evaluation.get("server_ms"),
                "vector_ms": evaluation.get("vector_ms"),
                "top_distance": evaluation.get("top_distance"),
                "top_chunk_id": top_chunk or None,
                "distance_threshold": evaluation.get("distance_threshold"),
                "min_similarity": evaluation.get("min_similarity"),
                "sqlx_fts_ms": evaluation.get("sqlx_fts_ms"),
                "weighted_chunk": weighted or None,
                "weighted_rank": evaluation.get("weighted_rank"),
                "flat_chunk": flat or None,
                "flat_rank": evaluation.get("flat_rank"),
                "tsvector_weights": evaluation.get("tsvector_weights"),
                "weight_values": evaluation.get("weight_values"),
                "status": ticket_body.get("status"),
                "department": ticket_body.get("department_name"),
                "guide_cited": guide or None,
                "outcome": outcome,
                "label": classify(ticket["expected"], outcome, guide),
                "ticket_id": ticket_body.get("id"),
                "vector_matches_expected": top_chunk.upper() == ticket["expected"],
                "weighted_matches_expected": weighted.upper() == ticket["expected"],
                "flat_matches_expected": flat.upper() == ticket["expected"],
                "weighted_differs_from_flat": weighted.upper() != flat.upper(),
            }
        )
        if ticket["saw_confirmo"] and ticket_body.get("id"):
            try:
                confirm_ms, confirmed = post_json(
                    base_url.rstrip("/") + "/api/client/message",
                    {"ticket_id": ticket_body["id"], "content": "CONFIRMO"},
                    timeout,
                )
                case = confirmed.get("support_case") or {}
                row["confirm_http_ms"] = round(confirm_ms, 3)
                row["case_code"] = case.get("case_code")
                row["case_status"] = case.get("status")
                row["case_guide_id"] = case.get("guide_id")
                row["case_created"] = bool(case.get("case_code"))
            except (urllib.error.URLError, TimeoutError, json.JSONDecodeError) as err:
                row["case_created"] = False
                row["case_error"] = str(err)
        rows.append(row)

    total = len(rows)
    deflected = sum(1 for row in rows if row.get("outcome") == "deflected")
    escalated = sum(1 for row in rows if row.get("outcome") == "escalated")
    labels = [row.get("label") for row in rows]
    matrix = {
        "tp": labels.count("tp"),
        "tn": labels.count("tn"),
        "fp": labels.count("fp"),
        "fn": labels.count("fn"),
        "unresolved": labels.count("unresolved"),
        "error": labels.count("error"),
    }
    confirmed = [row for row in rows if row.get("saw_confirmo")]
    cases_ok = sum(1 for row in confirmed if row.get("case_created"))
    vector_hits = sum(1 for row in rows if row.get("vector_matches_expected"))
    weighted_hits = sum(1 for row in rows if row.get("weighted_matches_expected"))
    http_values = [row["http_ms"] for row in rows if isinstance(row.get("http_ms"), (int, float))]
    sql_values = [
        row["sqlx_fts_ms"] for row in rows if isinstance(row.get("sqlx_fts_ms"), (int, float))
    ]
    vector_values = [
        row["vector_ms"] for row in rows if isinstance(row.get("vector_ms"), (int, float))
    ]
    return {
        "suite": "S14-S15",
        "base_url": base_url,
        "tickets": total,
        "deflected": deflected,
        "escalated": escalated,
        "deflection_rate_pct": round((deflected / total) * 100.0, 2) if total else 0.0,
        "confusion_matrix": matrix,
        "support_cases": {
            "archives_with_confirmo": len(confirmed),
            "rows_created": cases_ok,
        },
        "retrieval_agreement": {
            "vector_top_equals_expected": vector_hits,
            "weighted_tsvector_equals_expected": weighted_hits,
            "note": (
                "La columna plana (pesos 0.25) representa la búsqueda léxica sin "
                "priorizar título ni palabras clave. La columna ponderada es el "
                "tsvector nuevo: A título 1.0, B palabras clave 0.4, C cuerpo 0.2. "
                "El índice vectorial es la arquitectura Rust actual."
            ),
        },
        "latency_ms": {
            "http_avg": round(sum(http_values) / len(http_values), 3) if http_values else None,
            "http_max": round(max(http_values), 3) if http_values else None,
            "sqlx_fts_avg": round(sum(sql_values) / len(sql_values), 3) if sql_values else None,
            "vector_avg": round(sum(vector_values) / len(vector_values), 3)
            if vector_values
            else None,
        },
        "calibration": {
            "tsvector_config": "simple",
            "weights": {"A": "titulo", "B": "palabras_clave", "C": "cuerpo", "D": "sin_uso"},
            "weight_values": {"A": 1.0, "B": 0.4, "C": 0.2, "D": 0.1},
            "flat_weights": {"D": 0.25, "C": 0.25, "B": 0.25, "A": 0.25},
            "distance_threshold": rows[0].get("distance_threshold") if rows else 0.65,
            "min_similarity": rows[0].get("min_similarity") if rows else 0.35,
        },
        "rows": rows,
        "interpretation": (
            "Un caso deflectado exige que la respuesta cite una guía y que el ticket "
            "no quede en esperando_agente. Si Claude no está configurado, el centro "
            "de ayuda escala y la tasa de deflexión cae aunque el índice haya "
            "encontrado la guía. vector_top_equals_expected mide esa recuperación "
            "aparte de la deflexión."
        ),
    }


def write_reports(report: dict, json_path: Path) -> None:
    json_path.parent.mkdir(parents=True, exist_ok=True)
    json_path.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    csv_path = json_path.with_suffix(".csv")
    fields = [
        "file",
        "expected",
        "outcome",
        "label",
        "guide_cited",
        "status",
        "http_ms",
        "server_ms",
        "vector_ms",
        "top_distance",
        "top_chunk_id",
        "sqlx_fts_ms",
        "weighted_chunk",
        "weighted_rank",
        "flat_chunk",
        "flat_rank",
        "case_code",
        "case_created",
        "vector_matches_expected",
        "weighted_matches_expected",
    ]
    with csv_path.open("w", encoding="utf-8", newline="") as handle:
        writer = csv.DictWriter(handle, fieldnames=fields, extrasaction="ignore")
        writer.writeheader()
        writer.writerows(report["rows"])
    matrix_path = json_path.parent / "s15_deflection_matrix.json"
    matrix = {
        "deflection_rate_pct": report["deflection_rate_pct"],
        "deflected": report["deflected"],
        "escalated": report["escalated"],
        "tickets": report["tickets"],
        "confusion_matrix": report["confusion_matrix"],
        "support_cases": report["support_cases"],
        "retrieval_agreement": report["retrieval_agreement"],
        "rows": [
            {
                "file": row.get("file"),
                "expected": row.get("expected"),
                "outcome": row.get("outcome"),
                "label": row.get("label"),
                "guide_cited": row.get("guide_cited"),
                "case_code": row.get("case_code"),
                "case_created": row.get("case_created"),
            }
            for row in report["rows"]
        ],
    }
    matrix_path.write_text(json.dumps(matrix, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")


def main() -> int:
    parser = argparse.ArgumentParser(description="Evaluación por lotes S14/S15")
    parser.add_argument("--root", default="data/eval", help="Carpeta con ticket_archivado_*.txt")
    parser.add_argument("--base-url", default="http://127.0.0.1:8766")
    parser.add_argument("--self-check", action="store_true", help="Solo valida el parser")
    parser.add_argument("--limit", type=int, default=0, help="Máximo de archivos (0 = todos)")
    parser.add_argument("--timeout", type=float, default=40.0)
    parser.add_argument(
        "--report",
        default="reports/s14_latency_and_search_metrics.json",
    )
    args = parser.parse_args()
    root = Path(args.root)
    tickets = discover(root)
    if args.limit > 0:
        tickets = tickets[: args.limit]
    errors = self_check(tickets)
    print(f"Archivos: {len(tickets)}")
    for ticket in tickets:
        print(
            f"- {ticket['file']}: cliente={ticket['client_name']} "
            f"expect={ticket['expected']} soporte_descartado={ticket['support_turns']} "
            f"problema={ticket['problem'][:140]}"
        )
    if errors:
        print("Parser inválido:", file=sys.stderr)
        for error in errors:
            print(f"  {error}", file=sys.stderr)
        return 1
    if args.self_check:
        print("Parser OK: SOPORTE descartado y la falla inicial es solo del CLIENTE.")
        return 0
    report = evaluate_live(tickets, args.base_url, args.timeout)
    write_reports(report, Path(args.report))
    print(
        f"Deflexión: {report['deflection_rate_pct']}% "
        f"({report['deflected']}/{report['tickets']}) "
        f"matriz={report['confusion_matrix']} "
        f"casos={report['support_cases']}"
    )
    print(f"Reporte: {args.report}")
    if any(row.get("outcome") == "error" for row in report["rows"]):
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
