//! Rutas HTTP equivalentes a `app/main.py`.

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Path, RawQuery, Request, State};
use axum::http::{header, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use tower_http::services::ServeDir;

use crate::error::AppError;
use crate::models::{ChatRequest, ChatResponse, HealthResponse, ReindexRequest, ReindexResponse};
use crate::state::AppState;

pub fn router(state: Arc<AppState>) -> Router {
    let static_dir = state.settings.project_root.join("static");
    Router::new()
        .route("/", get(home))
        .route("/health", get(health))
        .route("/api/chat", post(chat))
        .route("/api/reindex", post(reindex))
        .route("/api/index", get(index_status))
        .route("/api/cases", get(list_cases))
        .route("/api/conversations", get(list_conversations))
        .route("/api/conversations/:session_id", get(get_conversation))
        .nest_service("/static", ServeDir::new(static_dir))
        .with_state(state)
}

async fn home(State(state): State<Arc<AppState>>) -> Result<impl IntoResponse, AppError> {
    let mut context = tera::Context::new();
    context.insert("assistant_name", "Asistente MI.COM.CO");
    let html = state
        .templates
        .render("index.html", &context)
        .map_err(|err| {
            AppError::internal(
                format!("No se pudo renderizar la interfaz: {err}"),
                "No se pudo cargar la interfaz.",
            )
        })?;
    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        html,
    ))
}

async fn health(State(state): State<Arc<AppState>>) -> Json<HealthResponse> {
    Json(state.health().await)
}

async fn chat(
    State(state): State<Arc<AppState>>,
    request: Request,
) -> Result<Json<ChatResponse>, AppError> {
    let bytes = read_body(request).await?;
    let mut payload: ChatRequest = serde_json::from_slice(&bytes)
        .map_err(|_| AppError::unprocessable("La solicitud no es válida."))?;
    payload.validate()?;
    let response = state
        .ask(payload.question, payload.history, payload.session_id)
        .await?;
    Ok(Json(response))
}

async fn reindex(
    State(state): State<Arc<AppState>>,
    request: Request,
) -> Result<Json<ReindexResponse>, AppError> {
    let bytes = read_body(request).await?;
    let options = if bytes.iter().all(u8::is_ascii_whitespace) {
        ReindexRequest::default()
    } else {
        serde_json::from_slice::<ReindexRequest>(&bytes)
            .map_err(|_| AppError::unprocessable("La solicitud no es válida."))?
    };
    tracing::info!(
        reset = options.reset,
        force = options.force,
        "endpoint /api/reindex"
    );
    Ok(Json(
        state.reindex(options.reset, options.force, None).await?,
    ))
}

async fn index_status(
    State(state): State<Arc<AppState>>,
) -> Result<Json<crate::models::IndexStatusResponse>, AppError> {
    Ok(Json(state.index_status().await?))
}

async fn list_cases(
    State(state): State<Arc<AppState>>,
    RawQuery(query): RawQuery,
) -> Result<Json<Vec<crate::cases::CaseRecord>>, AppError> {
    let limit = parse_limit(query.as_deref(), 20, 100)?;
    Ok(Json(state.list_cases(limit)?))
}

async fn list_conversations(
    State(state): State<Arc<AppState>>,
    RawQuery(query): RawQuery,
) -> Result<Json<Vec<crate::models::ConversationSummary>>, AppError> {
    let limit = parse_limit(query.as_deref(), 50, 200)?;
    Ok(Json(state.list_conversations(limit)?))
}

async fn get_conversation(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
) -> Result<Json<crate::models::ConversationDetail>, AppError> {
    Ok(Json(state.conversation(&session_id)?))
}

async fn read_body(request: Request) -> Result<Bytes, AppError> {
    axum::body::to_bytes(request.into_body(), 2 * 1024 * 1024)
        .await
        .map_err(|_| AppError::unprocessable("La solicitud no es válida."))
}

fn parse_limit(query: Option<&str>, default: i64, max: i64) -> Result<usize, AppError> {
    let Some(query) = query.filter(|value| !value.is_empty()) else {
        return Ok(default as usize);
    };
    for pair in query.split('&') {
        let mut parts = pair.splitn(2, '=');
        let key = parts.next().unwrap_or_default();
        if key != "limit" {
            continue;
        }
        let value = parts.next().unwrap_or_default();
        let parsed: i64 = value
            .parse()
            .map_err(|_| AppError::unprocessable("La solicitud no es válida."))?;
        return Ok(parsed.clamp(1, max) as usize);
    }
    Ok(default as usize)
}

#[cfg(test)]
mod tests {
    use super::router;
    use crate::config::Settings;
    use crate::rag::runtime_settings_for_tests;
    use crate::state::AppState;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use serde_json::Value;
    use std::sync::Arc;
    use tower::ServiceExt;

    async fn test_app(dir: &std::path::Path) -> axum::Router {
        std::fs::create_dir_all(dir.join("documents")).unwrap();
        let settings = runtime_settings_for_tests(dir, &Settings::load());
        let state = AppState::init(settings).await.unwrap();
        router(state)
    }

    async fn json_body(response: axum::response::Response) -> Value {
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    }

    #[tokio::test]
    async fn health_and_home_and_static() {
        let dir = std::env::temp_dir().join(format!("micomco-http-{}", uuid::Uuid::new_v4()));
        let app = test_app(&dir).await;

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
        let payload = json_body(health).await;
        assert!(payload.get("status").is_some());
        assert!(payload.get("vector_database").is_some());
        assert_eq!(payload["claude_api"], "missing_api_key");

        let home = app
            .clone()
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(home.status(), StatusCode::OK);
        let html = String::from_utf8(
            home.into_body()
                .collect()
                .await
                .unwrap()
                .to_bytes()
                .to_vec(),
        )
        .unwrap();
        assert!(html.contains("Soporte"));
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
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn chat_rejects_blank_question_and_missing_key() {
        let dir = std::env::temp_dir().join(format!("micomco-http-{}", uuid::Uuid::new_v4()));
        let app = test_app(&dir).await;
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

        let missing_key = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/chat")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"question":"Hola"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(missing_key.status(), StatusCode::SERVICE_UNAVAILABLE);
        let payload = json_body(missing_key).await;
        assert!(payload["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("ANTHROPIC_API_KEY"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn index_hides_templates_and_reindex_is_incremental() {
        let dir = std::env::temp_dir().join(format!("micomco-http-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("documents")).unwrap();
        std::fs::write(
            dir.join("documents/documento.txt"),
            "La política de la empresa indica soporte continuo.\n\nEl código de cliente es CLI-XXXXXX.",
        )
        .unwrap();
        std::fs::write(dir.join("documents/_plantilla_guia.txt"), "no indexar").unwrap();
        let settings = runtime_settings_for_tests(&dir, &Settings::load());
        let state: Arc<AppState> = AppState::init(settings).await.unwrap();
        let app = router(state.clone());

        let listed = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/index")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(listed.status(), StatusCode::OK);
        let payload = json_body(listed).await;
        let names: Vec<&str> = payload["files"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|item| item["name"].as_str())
            .collect();
        assert!(names.contains(&"documento.txt"));
        assert!(names.iter().all(|name| !name.starts_with('_')));

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
        let payload = json_body(reindex).await;
        assert_eq!(payload["status"], "ok");
        assert!(payload["chunks_indexed"].as_i64().unwrap_or(0) >= 1);

        std::fs::write(
            dir.join("documents/renovacion.txt"),
            "Para renovar un dominio entra a Mis servicios y revisa la fecha de vencimiento.",
        )
        .unwrap();
        let second = state.reindex(false, false, None).await.unwrap();
        assert_eq!(second.files_indexed, 1);
        assert!(second.sources.iter().any(|name| name == "renovacion.txt"));
        let third = state.reindex(false, false, None).await.unwrap();
        assert_eq!(third.files_indexed, 0);
        assert_eq!(third.files_skipped, 2);

        let cases = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/cases?limit=5")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(cases.status(), StatusCode::OK);

        let missing = app
            .oneshot(
                Request::builder()
                    .uri("/api/conversations/ses-no-existe")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
        let _ = std::fs::remove_dir_all(dir);
    }
}
