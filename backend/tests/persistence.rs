use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use micomco_backend::config::Config;
use micomco_backend::db;
use micomco_backend::domain::models::ChatRequest;
use micomco_backend::domain::models::CreateCaseRequest;
use micomco_backend::repositories::knowledge::KnowledgeRepository;
use micomco_backend::routes::router;
use micomco_backend::services::{case_service, chat_service, index_service};
use micomco_backend::state::AppState;
use serde_json::Value;
use sqlx::PgPool;
use tower::ServiceExt;

fn database_url() -> Option<String> {
    let url = std::env::var("MICOMCO_TEST_DATABASE_URL").ok()?;
    let url = url.trim().to_string();
    if url.is_empty() {
        return None;
    }
    if !url.contains("micomco_test") {
        panic!("MICOMCO_TEST_DATABASE_URL debe apuntar a una base llamada micomco_test");
    }
    Some(url)
}

#[tokio::test]
async fn persistence_search_chat_and_http() {
    let Some(url) = database_url() else {
        eprintln!("MICOMCO_TEST_DATABASE_URL no definida; se omite la prueba de persistencia.");
        return;
    };

    let pool = PgPool::connect(&url).await.expect("conexión de prueba");
    db::apply_migrations(&pool).await.expect("migraciones");
    sqlx::query(
        "TRUNCATE TABLE conversation_messages, conversations, support_cases, knowledge_chunks, knowledge_documents RESTART IDENTITY CASCADE",
    )
    .execute(&pool)
    .await
    .expect("limpieza");

    let state = AppState::new(pool, Config::for_tests(url));
    let indexed = index_service::reindex(&state, true, true)
        .await
        .expect("indexación");
    assert!(
        indexed.chunks_indexed >= 36,
        "chunks: {}",
        indexed.chunks_indexed
    );

    let again = index_service::reindex(&state, false, false)
        .await
        .expect("reindex incremental");
    assert_eq!(again.files_indexed, 0);
    assert!(again.files_skipped >= 1);

    let hits = KnowledgeRepository::new(&state.pool)
        .search("como cambiar la contraseña y activar 2FA", None, 8)
        .await
        .expect("búsqueda");
    assert!(
        hits.iter()
            .any(|hit| hit.doc_chunk_id.as_deref() == Some("GUA-003")),
        "no recuperó GUA-003: {:?}",
        hits.iter()
            .map(|hit| (hit.doc_chunk_id.clone(), hit.score, hit.title.clone()))
            .collect::<Vec<_>>()
    );

    let article = KnowledgeRepository::new(&state.pool)
        .by_doc_chunk_id("hst-004")
        .await
        .expect("artículo");
    assert!(article.iter().any(|hit| hit.content.contains("18")));

    let chat = chat_service::ask(
        &state,
        ChatRequest {
            question: "¿Cómo cambio la contraseña y activo 2FA?".into(),
            session_id: None,
            history: Vec::new(),
        },
    )
    .await
    .expect("chat");
    let session_id = chat.session_id.expect("sesión");
    assert!(chat.answer.contains("2FA") || chat.answer.contains("contraseña"));
    assert!(chat
        .sources
        .iter()
        .any(|source| source.doc_chunk_id.as_deref() == Some("GUA-003")));
    assert!(!chat.case_closed);

    let follow_up = chat_service::ask(
        &state,
        ChatRequest {
            question: "Quiero registrar un caso. Correo cliente@mi.com.co. Categoría dominio. No puedo transferir el dominio porque el antirrobo sigue activo. Confirmo.".into(),
            session_id: Some(session_id.clone()),
            history: Vec::new(),
        },
    )
    .await
    .expect("registro");
    assert!(follow_up.case_closed);
    let case_id = follow_up.case.expect("caso").case_id;
    assert!(case_id.starts_with("CASO-"));

    let created = case_service::create_case(
        &state,
        CreateCaseRequest {
            correo: "facturas@mi.com.co".into(),
            categoria: "factura".into(),
            detalle: "No llega la factura electrónica al correo".into(),
            session_id: None,
        },
    )
    .await
    .expect("caso http");
    assert_eq!(created.categoria, "Facturación o Compras");

    let app = router(state.clone());
    let response = app
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let health: Value = serde_json::from_slice(&bytes).expect("json");
    assert_eq!(health["status"], "ok");
    assert!(health["indexed_chunks"].as_i64().unwrap_or(0) >= 36);

    let app = router(state.clone());
    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/conversations/{session_id}"))
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let conversation: Value = serde_json::from_slice(&bytes).expect("json");
    assert_eq!(conversation["status"], "closed");
    assert!(conversation["message_count"].as_i64().unwrap_or(0) >= 2);

    let app = router(state.clone());
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/cases")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"correo":"mal","categoria":"dominio","detalle":"algo mas"}"#,
                ))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}
