use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("{0}")]
    BadRequest(String),
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Unprocessable(String),
    #[error("{0}")]
    Document(String),
    #[error("base de datos no disponible")]
    Database(#[source] sqlx::Error),
    #[error("{0}")]
    Internal(String),
}

impl From<sqlx::Error> for AppError {
    fn from(err: sqlx::Error) -> Self {
        AppError::Database(err)
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, detail) = match &self {
            AppError::BadRequest(message) | AppError::Document(message) => {
                (StatusCode::BAD_REQUEST, message.clone())
            }
            AppError::NotFound(message) => (StatusCode::NOT_FOUND, message.clone()),
            AppError::Unprocessable(message) => (StatusCode::UNPROCESSABLE_ENTITY, message.clone()),
            AppError::Database(err) => {
                tracing::error!(error = %err, "error de PostgreSQL");
                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    "La base de datos no está disponible. Inténtalo de nuevo.".to_string(),
                )
            }
            AppError::Internal(message) => {
                tracing::error!(error = %message, "error interno");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Ocurrió un error interno al procesar la solicitud.".to_string(),
                )
            }
        };
        if status.is_client_error() {
            tracing::debug!(%detail, "solicitud rechazada");
        }
        (status, Json(serde_json::json!({ "detail": detail }))).into_response()
    }
}

#[derive(Debug)]
pub struct StartupError {
    message: String,
}

impl StartupError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub fn with_source_message(mut self, err: impl std::fmt::Display) -> Self {
        tracing::error!(error = %err, "{}", self.message);
        self.message = format!("{}. Detalle: {err}", self.message);
        self
    }
}

impl std::fmt::Display for StartupError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for StartupError {}
