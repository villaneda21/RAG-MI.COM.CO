//! Contratos HTTP que el frontend (`static/js/app.js`) ya consume.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use rag_micomco::assemble_rag;
use rag_micomco::config::Settings;
use rag_micomco::embed::HashEmbedder;
use rag_micomco::routes::{router, AppState};
use serde_json::Value;
use tempfile::tempdir;
use tower::ServiceExt;

fn test_app(dir: &std::path::Path) -> axum::Router {
    std::fs::write(
        dir.join("documento.txt"),
        "La política de la empresa indica que el soporte técnico responde en horario continuo.\n\n\
El código de cliente tiene el formato CLI-XXXXXX y se consulta en Mi cuenta.\n",
    )
    .unwrap();
    std::fs::write(dir.join("_plantilla_guia.txt"), "no indexar").unwrap();
    let settings = Settings::for_tests(dir);
    let rag = assemble_rag(settings, Arc::new(HashEmbedder { dimensions: 32 }), None);
    let state = AppState::try_new(rag).unwrap();
    router(state)
}

async fn body_json(response: axum::response::Response) -> Value {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
}

#[tokio::test]
async fn home_renders_chatbot_and_static_css() {
    let dir = tempdir().unwrap();
    let app = test_app(dir.path());
    let response = app
        .clone()
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let html = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(html.contains("Soporte") || html.contains("Asistente"));
    assert!(html.contains("Historial"));
    assert!(html.contains("Base"));
    assert!(html.contains("/static/css/style.css"));

    let css = app
        .oneshot(
            Request::builder()
                .uri("/static/css/style.css")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(css.status(), StatusCode::OK);
}

#[tokio::test]
async fn health_reindex_index_and_chat_validation() {
    let dir = tempdir().unwrap();
    let app = test_app(dir.path());

    let health = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(health.status(), StatusCode::OK);
    let payload = body_json(health).await;
    assert!(payload.get("status").is_some());
    assert!(payload.get("vector_database").is_some());

    let blank = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/chat")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"question":"   "}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(blank.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let detail = body_json(blank).await;
    assert_eq!(detail["detail"], "La solicitud no es válida.");

    let missing_key = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/chat")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"question":"¿Cómo creo una cuenta?"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing_key.status(), StatusCode::SERVICE_UNAVAILABLE);
    let detail = body_json(missing_key).await;
    assert!(
        detail["detail"]
            .as_str()
            .unwrap_or("")
            .contains("ANTHROPIC_API_KEY")
            || detail["detail"].as_str().unwrap_or("").contains("clave")
    );

    let reindex = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/reindex")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"reset":true}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(reindex.status(), StatusCode::OK);
    let reindex_body = body_json(reindex).await;
    assert_eq!(reindex_body["status"], "ok");
    assert!(reindex_body["chunks_indexed"].as_u64().unwrap_or(0) >= 1);

    let index = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/index")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(index.status(), StatusCode::OK);
    let index_body = body_json(index).await;
    let names = index_body["files"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|item| item.get("name").and_then(|name| name.as_str()))
        .collect::<Vec<_>>();
    assert!(names.iter().all(|name| !name.starts_with('_')));
    assert!(names.contains(&"documento.txt"));

    let cases = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/cases?limit=20")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(cases.status(), StatusCode::OK);
    assert!(body_json(cases).await.as_array().unwrap().is_empty());

    let missing = app
        .oneshot(
            Request::builder()
                .uri("/api/conversations/ses-no-existe-123")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
}
