import os
import re
import glob
import time
import json
import csv
import requests
from datetime import datetime

# ==========================================
# CONFIGURACIÓN DEL ENTORNO DE PRUEBAS
# ==========================================
BASE_URL = os.getenv("API_URL", "http://localhost:8000")
TICKETS_DIR = os.getenv("TICKETS_DIR", "./data/tickets")
REPORTS_DIR = "./reports"

GREEN = "\033[92m"
YELLOW = "\033[93m"
RED = "\033[91m"
CYAN = "\033[96m"
BOLD = "\033[1m"
RESET = "\033[0m"

WORKING_TRIAGE_KEYSET = None

def parse_ticket_file(file_path):
    with open(file_path, "r", encoding="utf-8", errors="ignore") as f:
        content = f.read()

    blocks = content.split("-----------------------------------------------------------------")
    client_messages = []
    client_name = "Cliente Evaluación"

    for block in blocks:
        lines = [line.strip() for line in block.strip().split("\n") if line.strip()]
        if not lines:
            continue
        
        header = lines[0]
        if "👤 CLIENTE" in header:
            match_name = re.search(r"👤 CLIENTE \((.*?)\)", header)
            if match_name:
                client_name = match_name.group(1)
                
            for line in lines[1:]:
                if line.startswith("💬"):
                    text = line.replace("💬", "").strip()
                    if text:
                        client_messages.append(text)

    full_problem = " ".join(client_messages) if client_messages else None
    return client_name, full_problem

def send_triage_autodetect(client_name, email, domain, problem_text):
    global WORKING_TRIAGE_KEYSET

    headers = {"Content-Type": "application/json"}

    # Lista ampliada de variantes estructurales para Axum/Serde
    candidate_payloads = [
        {"name": client_name, "email_micomco": email, "domain": domain, "problem_description": problem_text},
        {"name": client_name, "email": email, "domain": domain, "description": problem_text},
        {"name": client_name, "email_micomco": email, "domain": domain, "description": problem_text},
        {"name": client_name, "email": email, "domain": domain, "problem_description": problem_text},
        {"name": client_name, "email": email, "domain": domain, "subject": "Soporte Técnico", "issue": problem_text},
        {"name": client_name, "email_micomco": email, "domain": domain, "subject": "Falla Reportada", "message": problem_text},
        {"name": client_name, "email": email, "domain": domain, "category": "Hosting", "description": problem_text},
        {"name": client_name, "email_micomco": email, "domain": domain, "category": "Soporte", "problem_description": problem_text},
    ]

    if WORKING_TRIAGE_KEYSET is not None:
        payload = candidate_payloads[WORKING_TRIAGE_KEYSET]
        return requests.post(f"{BASE_URL}/api/client/triage", json=payload, headers=headers, timeout=10)

    for idx, payload in enumerate(candidate_payloads):
        res = requests.post(f"{BASE_URL}/api/client/triage", json=payload, headers=headers, timeout=10)
        if res.status_code == 200:
            WORKING_TRIAGE_KEYSET = idx
            print(f"{GREEN}✔ Esquema de Triage identificado (Variante #{idx+1}){RESET}")
            return res
            
    return res

def run_evaluation():
    os.makedirs(REPORTS_DIR, exist_ok=True)
    
    ticket_files = glob.glob(os.path.join(TICKETS_DIR, "ticket_archivado_*.txt"))
    if not ticket_files:
        ticket_files = glob.glob("ticket_archivado_*.txt")

    print(f"\n{CYAN}{BOLD}========================================================")
    print("  MI.COM.CO — EVALUACIÓN OFFLINE DE DEFLEXIÓN Y LATENCIA")
    print(f"========================================================{RESET}\n")
    print(f"📁 Archivos encontrados: {BOLD}{len(ticket_files)}{RESET}")
    print(f"🔗 Conectando a Backend Rust: {BOLD}{BASE_URL}{RESET}\n")

    if not ticket_files:
        print(f"{RED}❌ No se encontraron archivos 'ticket_archivado_*.txt'.{RESET}")
        return

    results = []
    total_tickets = len(ticket_files)
    deflected_count = 0
    escalated_count = 0
    total_latencies = []

    for idx, filepath in enumerate(ticket_files, 1):
        filename = os.path.basename(filepath)
        client_name, problem_text = parse_ticket_file(filepath)

        if not problem_text:
            print(f"[{idx}/{total_tickets}] {YELLOW}⚠️ {filename}: Sin mensajes de cliente válidos.{RESET}")
            continue

        email_test = f"cliente{idx}@mi.com.co"
        domain_test = "mi-dominio.com.co"
        problem_snippet = problem_text[:120]

        try:
            t1 = time.time()
            triage_res = send_triage_autodetect(client_name, email_test, domain_test, problem_snippet)
            
            if triage_res.status_code != 200:
                print(f"[{idx}/{total_tickets}] {RED}❌ {filename}: Error Triage (HTTP {triage_res.status_code}) -> {triage_res.text}{RESET}")
                continue

            ticket_data = triage_res.json()
            ticket_id = ticket_data.get("ticket_id") or ticket_data.get("id") or ticket_data.get("data", {}).get("ticket_id")

            msg_payload = {
                "ticket_id": str(ticket_id),
                "content": problem_text,
                "media_type": "text"
            }

            msg_res = requests.post(f"{BASE_URL}/api/client/message", json=msg_payload, headers={"Content-Type": "application/json"}, timeout=25)
            latency_ms = round((time.time() - t1) * 1000, 2)
            total_latencies.append(latency_ms)

            if msg_res.status_code == 200:
                response_data = msg_res.json()
                bot_reply = response_data.get("reply") or response_data.get("message") or response_data.get("content") or ""
                status = response_data.get("status", "en_bot")
                dept = response_data.get("department", "Soporte Técnico")

                is_deflected = ("GUA-" in bot_reply or status in ["resuelto", "cerrado"]) and status != "esperando_agente"

                if is_deflected:
                    deflected_count += 1
                    status_badge = f"{GREEN}[DEFLECTADO - AUTOSERVICIO]{RESET}"
                else:
                    escalated_count += 1
                    status_badge = f"{YELLOW}[ESCALADO -> {dept}]{RESET}"

                print(f"[{idx}/{total_tickets}] {status_badge} {filename} ({latency_ms} ms)")
                print(f"   👤 Cliente: {problem_text[:70]}...")
                print(f"   🤖 Bot: {bot_reply[:80].replace('\n', ' ')}...\n")

                results.append({
                    "ticket_file": filename,
                    "ticket_id": str(ticket_id),
                    "client_name": client_name,
                    "latency_ms": latency_ms,
                    "status": status,
                    "department": dept,
                    "is_deflected": is_deflected,
                    "prompt_input": problem_text,
                    "bot_response": bot_reply
                })
            else:
                print(f"[{idx}/{total_tickets}] {RED}❌ {filename}: Error Mensaje (HTTP {msg_res.status_code}) -> {msg_res.text}{RESET}")

        except Exception as e:
            print(f"[{idx}/{total_tickets}] {RED}❌ {filename}: Excepción -> {e}{RESET}")

    eval_count = len(results)
    deflection_rate = round((deflected_count / eval_count) * 100, 2) if eval_count > 0 else 0.0
    avg_latency = round(sum(total_latencies) / len(total_latencies), 2) if total_latencies else 0.0
    min_latency = min(total_latencies) if total_latencies else 0.0
    max_latency = max(total_latencies) if total_latencies else 0.0

    print(f"\n{CYAN}{BOLD}========================================================")
    print("                RESUMEN DE EVALUACIÓN")
    print(f"========================================================{RESET}")
    print(f" Total Tickets Evaluados:  {BOLD}{eval_count}{RESET}")
    print(f" Casos Deflectados (Bot): {GREEN}{BOLD}{deflected_count}{RESET}")
    print(f" Casos Escalados (Humano): {YELLOW}{BOLD}{escalated_count}{RESET}")
    print(f" {BOLD}TASA DE DEFLEXIÓN:        {GREEN}{deflection_rate}%{RESET}")
    print(f" Latencia Promedio:        {avg_latency} ms (Min: {min_latency}ms | Max: {max_latency}ms)")
    print(f"{CYAN}========================================================{RESET}\n")

    json_path = os.path.join(REPORTS_DIR, "s14_s15_eval_results.json")
    summary_data = {
        "timestamp": datetime.now().isoformat(),
        "metrics": {
            "total_evaluated": eval_count,
            "deflected_count": deflected_count,
            "escalated_count": escalated_count,
            "deflection_rate_percentage": deflection_rate,
            "latency_avg_ms": avg_latency,
            "latency_min_ms": min_latency,
            "latency_max_ms": max_latency
        },
        "details": results
    }
    
    with open(json_path, "w", encoding="utf-8") as f:
        json.dump(summary_data, f, indent=2, ensure_ascii=False)

    csv_path = os.path.join(REPORTS_DIR, "s14_s15_eval_summary.csv")
    if results:
        keys = ["ticket_file", "ticket_id", "client_name", "latency_ms", "status", "department", "is_deflected", "prompt_input", "bot_response"]
        with open(csv_path, "w", newline="", encoding="utf-8") as f:
            dict_writer = csv.DictWriter(f, fieldnames=keys)
            dict_writer.writeheader()
            dict_writer.writerows(results)

    print(f"✅ Reporte JSON guardado en: {BOLD}{json_path}{RESET}")
    print(f"✅ Reporte CSV guardado en:  {BOLD}{csv_path}{RESET}\n")

if __name__ == "__main__":
    run_evaluation()