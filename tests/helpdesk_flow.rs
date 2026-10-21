//! Flujo del centro de ayuda contra PostgreSQL de prueba.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use rag_micomco::assemble_rag;
use rag_micomco::cases::CaseService;
use rag_micomco::claude::{ClaudeServiceError, SupportClient, SupportTurn};
use rag_micomco::config::Settings;
use rag_micomco::conversations::HistoryTurn;
use rag_micomco::embed::HashEmbedder;
use rag_micomco::helpdesk::{run_inactivity_pass, Helpdesk};
use rag_micomco::routes::{router, AppState};
use rag_micomco::state::AppState as StateWithDesk;
use serde_json::{json, Value};
use tempfile::tempdir;
use tower::ServiceExt;

const TEST_DB: &str = "postgres://rag:rag@127.0.0.1:5432/rag_micomco_test?sslmode=disable";

struct ScriptedClaude;

static SEEN_SESSION: Mutex<String> = Mutex::new(String::new());

#[async_trait]
impl SupportClient for ScriptedClaude {
    fn is_configured(&self) -> bool {
        true
    }

    async fn generate_support_reply(
        &self,
        question: String,
        _history: Vec<HistoryTurn>,
        _session_id: String,
        _case_service: CaseService,
        _context: String,
        session_note: String,
    ) -> Result<SupportTurn, ClaudeServiceError> {
        *SEEN_SESSION.lock().unwrap_or_else(|err| err.into_inner()) = session_note;
        let folded = question.to_lowercase();
        let text = if folded.contains("ssl") || folded.contains("certificado") {
            "[[SIN_COBERTURA]] No está en la base de conocimiento.".to_string()
        } else {
            "Revisa el código en Mi cuenta [guía](https://mi.com.co/cuenta).\n\
Consulta https://docs.example.com/cli.\n\
\n\
Fuentes:\n\
- documento.txt\n"
                .to_string()
        };
        Ok(SupportTurn { text, case: None })
    }
}

fn message_text(body: &Value, sender: &str) -> String {
    body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["sender_type"] == sender)
        .filter_map(|message| message["content"].as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

async fn json_body(response: axum::response::Response) -> Value {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
}

fn post(uri: &str, payload: Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(payload.to_string()))
        .unwrap()
}

fn put(uri: &str, payload: Value) -> Request<Body> {
    Request::builder()
        .method("PUT")
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(payload.to_string()))
        .unwrap()
}

#[tokio::test]
async fn helpdesk_triage_crm_and_inactivity() {
    let dir = tempdir().unwrap();
    std::fs::write(dir.path().join("documento.txt"), "guia de soporte\n").unwrap();
    let media = dir.path().join("media");
    let helpdesk = Helpdesk::connect(TEST_DB, media.clone())
        .await
        .expect("PostgreSQL de prueba");
    sqlx::query("TRUNCATE messages, ticket_audit_logs, tickets, customers CASCADE")
        .execute(&helpdesk.pool)
        .await
        .unwrap();
    let original_rules = helpdesk.rules().await.unwrap();

    let settings = Settings::for_tests(dir.path());
    let rag = assemble_rag(
        settings,
        Arc::new(HashEmbedder { dimensions: 32 }),
        Some(Arc::new(ScriptedClaude)),
    );
    let state: StateWithDesk = AppState::try_with_helpdesk(rag, Some(helpdesk.clone())).unwrap();
    let app = router(state);

    let home = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/client")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(home.status(), StatusCode::OK);
    let home_html = String::from_utf8(
        home.into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
    .unwrap();
    assert!(home_html.contains("sender_type === 'system'"));
    assert!(home_html.contains("justify-center"));
    assert!(home_html.contains("Caso resuelto"));
    assert!(!home_html.contains("fixed inset-0"));
    assert!(home_html.contains("triage-form"));
    assert!(home_html.contains("Nuevo Chat de Prueba"));
    assert!(home_html.contains("Chats activos de prueba"));
    let crm = app
        .clone()
        .oneshot(Request::builder().uri("/crm").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(crm.status(), StatusCode::OK);

    let triage = app
        .clone()
        .oneshot(post(
            "/api/client/triage",
            json!({
                "name": "Ana Cliente",
                "domain": "tienda.test",
                "email_micomco": "ana@mi.com.co",
                "reason": "tengo una falla en el correo"
            }),
        ))
        .await
        .unwrap();
    assert_eq!(triage.status(), StatusCode::OK);
    let opened = json_body(triage).await;
    assert_eq!(opened["ticket"]["department_name"], "Soporte Técnico");
    assert_eq!(opened["ticket"]["priority"], "alta");
    assert_eq!(opened["ticket"]["status"], "en_bot");
    let bot = opened["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["sender_type"] == "bot")
        .unwrap();
    let answer = bot["content"].as_str().unwrap();
    assert!(!answer.contains("¿Pude resolver tu duda"));
    assert!(!answer.to_lowercase().contains("correo"));
    assert!(!answer.to_lowercase().contains("http"));
    assert!(!answer.to_lowercase().contains("fuentes"));
    assert!(answer.contains("Mi cuenta"));
    let seen = SEEN_SESSION
        .lock()
        .unwrap_or_else(|err| err.into_inner())
        .clone();
    assert!(seen.contains("ana@mi.com.co"));
    assert!(seen.contains("tienda.test"));
    assert!(seen.contains("NUNCA le vuelvas a pedir"));
    let ticket_id = opened["ticket"]["id"].as_str().unwrap();

    let thanks = app
        .clone()
        .oneshot(post(
            "/api/client/message",
            json!({ "ticket_id": ticket_id, "content": "Sí, gracias" }),
        ))
        .await
        .unwrap();
    assert_eq!(thanks.status(), StatusCode::OK);
    let thanks_body = json_body(thanks).await;
    assert_eq!(thanks_body["ticket"]["status"], "en_bot");
    assert!(!message_text(&thanks_body, "bot").contains("¿Pude resolver tu duda"));

    let closed = app
        .clone()
        .oneshot(put(
            &format!("/api/crm/tickets/{ticket_id}/status"),
            json!({ "status": "cerrado", "agent_id": null }),
        ))
        .await
        .unwrap();
    assert_eq!(closed.status(), StatusCode::OK);
    let closed_view = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/client/tickets/{ticket_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let closed_body = json_body(closed_view).await;
    assert_eq!(closed_body["ticket"]["status"], "cerrado");
    assert!(message_text(&closed_body, "system").contains("RESUELTO"));

    let reopened = app
        .clone()
        .oneshot(post(
            "/api/client/message",
            json!({
                "ticket_id": ticket_id,
                "content": "sigue sin llegar el correo"
            }),
        ))
        .await
        .unwrap();
    assert_eq!(reopened.status(), StatusCode::OK);
    let reopened_body = json_body(reopened).await;
    let follow_id = reopened_body["ticket"]["id"].as_str().unwrap().to_string();
    assert_ne!(follow_id, ticket_id);
    assert_eq!(reopened_body["ticket"]["status"], "en_bot");
    assert_eq!(
        reopened_body["ticket"]["department_name"],
        "Soporte Técnico"
    );
    let notice = message_text(&reopened_body, "system");
    assert!(notice.contains("nueva solicitud"));
    assert!(notice.contains('#'));
    assert!(reopened_body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|message| message["sender_type"] == "client"
            && message["content"] == "sigue sin llegar el correo"));
    let previous = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/client/tickets/{ticket_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let previous_body = json_body(previous).await;
    assert_eq!(previous_body["ticket"]["status"], "cerrado");
    assert!(previous_body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .all(|message| message["content"] != "sigue sin llegar el correo"));

    let marked = app
        .clone()
        .oneshot(put(
            &format!("/api/crm/tickets/{follow_id}/status"),
            json!({ "status": "resuelto", "agent_id": null }),
        ))
        .await
        .unwrap();
    assert_eq!(marked.status(), StatusCode::OK);
    assert_eq!(json_body(marked).await["ticket"]["status"], "resuelto");
    let marked_view = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/client/tickets/{follow_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let marked_body = json_body(marked_view).await;
    assert!(message_text(&marked_body, "system").contains("RESUELTO"));

    let ssl = app
        .clone()
        .oneshot(post(
            "/api/client/triage",
            json!({
                "name": "Clara SSL",
                "domain": "ssl.test",
                "email_micomco": "clara@mi.com.co",
                "reason": "como instalo el certificado SSL en cPanel"
            }),
        ))
        .await
        .unwrap();
    assert_eq!(ssl.status(), StatusCode::OK);
    let ssl_body = json_body(ssl).await;
    assert_eq!(ssl_body["ticket"]["status"], "esperando_agente");
    assert_eq!(ssl_body["ticket"]["department_name"], "Soporte Técnico");
    let ssl_notice = message_text(&ssl_body, "system");
    assert!(ssl_notice.contains("He transferido tu caso"));
    assert!(ssl_notice.contains("Soporte Técnico"));
    assert!(ssl_notice.contains('#'));
    assert_eq!(
        ssl_body["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|message| message["sender_type"] == "system")
            .count(),
        1
    );
    assert!(ssl_body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .all(|message| {
            let content = message["content"].as_str().unwrap_or("");
            !content.contains("¿Pude resolver tu duda")
                && !content.to_lowercase().contains("correo")
                && !content.contains("[[SIN_COBERTURA]]")
        }));
    let ssl_id = ssl_body["ticket"]["id"].as_str().unwrap();
    let ssl_follow = app
        .clone()
        .oneshot(post(
            "/api/client/message",
            json!({
                "ticket_id": ssl_id,
                "content": "sigues ahi"
            }),
        ))
        .await
        .unwrap();
    let ssl_follow_body = json_body(ssl_follow).await;
    assert_eq!(ssl_follow_body["ticket"]["status"], "esperando_agente");
    assert_eq!(
        ssl_follow_body["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|message| message["sender_type"] == "bot")
            .count(),
        0
    );
    assert_eq!(
        ssl_follow_body["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|message| message["sender_type"] == "system")
            .count(),
        1
    );

    let human = app
        .clone()
        .oneshot(post(
            "/api/client/triage",
            json!({
                "name": "Hugo Humano",
                "domain": "humano.test",
                "email_micomco": "hugo@mi.com.co",
                "reason": "comunícame con soporte"
            }),
        ))
        .await
        .unwrap();
    let human_body = json_body(human).await;
    assert_eq!(human_body["ticket"]["status"], "esperando_agente");
    assert_eq!(human_body["ticket"]["department_name"], "Soporte Técnico");
    assert_eq!(
        human_body["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|message| message["sender_type"] == "system")
            .count(),
        1
    );
    assert!(message_text(&human_body, "system").contains("He transferido tu caso"));

    let billing = app
        .clone()
        .oneshot(post(
            "/api/client/triage",
            json!({
                "name": "Luis Cliente",
                "domain": "pagos.test",
                "email_micomco": "luis@mi.com.co",
                "reason": "necesito la factura de marzo"
            }),
        ))
        .await
        .unwrap();
    assert_eq!(billing.status(), StatusCode::OK);
    let billing_body = json_body(billing).await;
    assert_eq!(billing_body["ticket"]["department_name"], "Facturación");
    assert_eq!(billing_body["ticket"]["priority"], "normal");
    let billing_id = billing_body["ticket"]["id"].as_str().unwrap().to_string();

    let sticker = app
        .clone()
        .oneshot(post(
            "/api/client/message",
            json!({
                "ticket_id": billing_id,
                "content": "",
                "mime_type": "image/gif",
                "filename": "sticker.gif",
                "media_base64": "R0lGODlhAQABAAAAACw="
            }),
        ))
        .await
        .unwrap();
    assert_eq!(sticker.status(), StatusCode::BAD_REQUEST);
    let sticker_body = json_body(sticker).await;
    assert!(sticker_body["detail"]
        .as_str()
        .unwrap()
        .contains("no se permiten archivos GIF"));

    let photo = app
        .clone()
        .oneshot(post(
            "/api/client/message",
            json!({
                "ticket_id": billing_id,
                "content": "mira esta captura",
                "mime_type": "image/png",
                "filename": "captura.png",
                "media_base64": "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg=="
            }),
        ))
        .await
        .unwrap();
    assert_eq!(photo.status(), StatusCode::OK);
    let photo_body = json_body(photo).await;
    let image = photo_body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["media_type"] == "image")
        .unwrap();
    let image_url = image["media_url"].as_str().unwrap();
    assert!(image_url.starts_with("/media/"));
    assert!(image_url.ends_with(".png"));
    assert!(media
        .join(image_url.trim_start_matches("/media/"))
        .is_file());

    let voice = app
        .clone()
        .oneshot(post(
            "/api/client/message",
            json!({
                "ticket_id": billing_id,
                "content": "te dejo el audio",
                "mime_type": "audio/ogg",
                "filename": "nota.ogg",
                "media_base64": "T2dnUw=="
            }),
        ))
        .await
        .unwrap();
    assert_eq!(voice.status(), StatusCode::OK);
    let voice_body = json_body(voice).await;
    let audio = voice_body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["media_type"] == "audio")
        .unwrap();
    let media_url = audio["media_url"].as_str().unwrap();
    assert!(media_url.starts_with("/media/"));
    assert!(media
        .join(media_url.trim_start_matches("/media/"))
        .is_file());

    let agents = json_body(
        app.clone()
            .oneshot(
                Request::builder()
                    .uri("/api/crm/agents")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    let agent_id = agents["agents"][0]["id"].as_str().unwrap();
    let sales = agents["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|agent| agent["department_name"] == "Ventas")
        .unwrap();
    let sales_agent = sales["id"].as_str().unwrap();
    let sales_department = sales["department_id"].as_str().unwrap();

    let queue = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/crm/tickets?view=bot&agent_id={agent_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(queue.status(), StatusCode::OK);
    let queue_body = json_body(queue).await;
    assert!(queue_body["tickets"]
        .as_array()
        .unwrap()
        .iter()
        .any(|ticket| ticket["id"] == billing_id));

    let note = app
        .clone()
        .oneshot(post(
            &format!("/api/crm/tickets/{billing_id}/notes"),
            json!({ "content": "Revisar NIT antes de responder.", "agent_id": agent_id }),
        ))
        .await
        .unwrap();
    assert_eq!(note.status(), StatusCode::OK);
    let note_body = json_body(note).await;
    assert_eq!(note_body["message"]["is_internal_note"], true);

    let client_view = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/client/tickets/{billing_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let client_body = json_body(client_view).await;
    assert!(client_body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .all(|message| message["is_internal_note"] != true));

    let transfer = app
        .clone()
        .oneshot(put(
            &format!("/api/crm/tickets/{billing_id}/transfer"),
            json!({
                "department_id": sales_department,
                "agent_id": sales_agent,
                "actor_agent_id": agent_id
            }),
        ))
        .await
        .unwrap();
    assert_eq!(transfer.status(), StatusCode::OK);
    let transfer_body = json_body(transfer).await;
    assert_eq!(transfer_body["ticket"]["department_name"], "Ventas");
    assert_eq!(transfer_body["ticket"]["status"], "en_proceso");
    let sales_name = sales["name"].as_str().unwrap();
    let transferred_view = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/client/tickets/{billing_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let transferred_body = json_body(transferred_view).await;
    let system_copy = message_text(&transferred_body, "system");
    assert!(system_copy.contains("transferido al área de Ventas"));
    assert!(system_copy.contains("asumido"));
    assert!(system_copy.contains(sales_name));

    let support = agents["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|agent| agent["name"] == "Agente Soporte")
        .unwrap();
    let moved_again = app
        .clone()
        .oneshot(put(
            &format!("/api/crm/tickets/{billing_id}/transfer"),
            json!({
                "department_id": support["department_id"],
                "agent_id": support["id"],
                "actor_agent_id": agent_id
            }),
        ))
        .await
        .unwrap();
    assert_eq!(moved_again.status(), StatusCode::OK);
    let after_second = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/client/tickets/{billing_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let after_second_body = json_body(after_second).await;
    let assumptions: Vec<_> = after_second_body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| {
            message["sender_type"] == "system"
                && message["content"]
                    .as_str()
                    .unwrap_or("")
                    .contains("ha asumido")
        })
        .collect();
    assert_eq!(assumptions.len(), 1);
    assert!(assumptions[0]["content"]
        .as_str()
        .unwrap()
        .contains("Agente Soporte"));

    let toggled = app
        .clone()
        .oneshot(put(
            &format!("/api/crm/tickets/{billing_id}/timeout-toggle"),
            json!({ "auto_timeout_enabled": false, "agent_id": agent_id }),
        ))
        .await
        .unwrap();
    assert_eq!(toggled.status(), StatusCode::OK);
    assert_eq!(
        json_body(toggled).await["ticket"]["auto_timeout_enabled"],
        false
    );

    let updated_customer = app
        .clone()
        .oneshot(put(
            "/api/crm/customers",
            json!({
                "id": billing_body["ticket"]["customer_id"],
                "name": "Luis Pagos",
                "domain": "pagos.test",
                "email_micomco": "luis@mi.com.co"
            }),
        ))
        .await
        .unwrap();
    assert_eq!(updated_customer.status(), StatusCode::OK);
    assert_eq!(json_body(updated_customer).await["name"], "Luis Pagos");

    let created = app
        .clone()
        .oneshot(post(
            "/api/admin/snippets",
            json!({
                "shortcut": "hola",
                "title": "Saludo",
                "content": "Hola, te escribe el equipo de MI.COM.CO.",
                "department_id": null
            }),
        ))
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::CREATED);
    let snippet_id = json_body(created).await["id"].as_str().unwrap().to_string();
    let edited = app
        .clone()
        .oneshot(put(
            &format!("/api/admin/snippets/{snippet_id}"),
            json!({
                "shortcut": "/hola",
                "title": "Saludo corto",
                "content": "Hola, gracias por escribir.",
                "department_id": null
            }),
        ))
        .await
        .unwrap();
    assert_eq!(edited.status(), StatusCode::OK);
    let removed = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/admin/snippets/{snippet_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(removed.status(), StatusCode::OK);

    let rules = app
        .clone()
        .oneshot(put(
            "/api/admin/rules",
            json!({
                "inactivity_warning_minutes": 0,
                "inactivity_close_minutes": 0,
                "warning_message_template": "Aviso de prueba por inactividad.",
                "close_message_template": "Cierre de prueba por inactividad.",
                "auto_close_enabled": true
            }),
        ))
        .await
        .unwrap();
    assert_eq!(rules.status(), StatusCode::OK);

    let idle = app
        .clone()
        .oneshot(post(
            "/api/client/triage",
            json!({
                "name": "Nico Cliente",
                "domain": "idle.test",
                "email_micomco": "nico@mi.com.co",
                "reason": "hola, no carga mi sitio"
            }),
        ))
        .await
        .unwrap();
    let idle_body = json_body(idle).await;
    let idle_id = idle_body["ticket"]["id"].as_str().unwrap();
    sqlx::query(
        "UPDATE tickets SET auto_timeout_enabled = FALSE WHERE id <> $1 AND status <> 'cerrado'",
    )
    .bind(uuid::Uuid::parse_str(idle_id).unwrap())
    .execute(&helpdesk.pool)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE tickets SET last_client_message_at = NOW() - INTERVAL '30 minutes', warning_sent_at = NULL WHERE id = $1",
    )
    .bind(uuid::Uuid::parse_str(idle_id).unwrap())
    .execute(&helpdesk.pool)
    .await
    .unwrap();

    run_inactivity_pass(&helpdesk).await.unwrap();
    let warned = helpdesk
        .ticket_by_id(uuid::Uuid::parse_str(idle_id).unwrap())
        .await
        .unwrap();
    assert_eq!(warned.status, "en_bot");
    assert!(warned.warning_sent_at.is_some());
    run_inactivity_pass(&helpdesk).await.unwrap();
    let shut = helpdesk
        .ticket_by_id(uuid::Uuid::parse_str(idle_id).unwrap())
        .await
        .unwrap();
    assert_eq!(shut.status, "cerrado");
    let audit = helpdesk
        .audit_log(uuid::Uuid::parse_str(idle_id).unwrap())
        .await
        .unwrap();
    assert!(audit
        .iter()
        .any(|entry| entry.action_type == "auto_timeout"));

    helpdesk.save_rules(&original_rules).await.unwrap();
}
