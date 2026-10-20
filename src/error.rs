//! Errores de dominio y su traducción a respuestas HTTP de Axum.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("{message}")]
    Document {
        message: String,
        user_message: String,
    },
    #[error("{message}")]
    Vector {
        message: String,
        user_message: String,
    },
    #[error("{message}")]
    Claude {
        message: String,
        user_message: String,
    },
    #[error("{0}")]
    BadRequest(String),
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Unprocessable(String),
    #[error("{message}")]
    Internal {
        message: String,
        user_message: String,
    },
}

impl AppError {
    pub fn document(message: impl Into<String>, user_message: impl Into<String>) -> Self {
        Self::Document {
            message: message.into(),
            user_message: user_message.into(),
        }
    }

    pub fn vector(message: impl Into<String>, user_message: impl Into<String>) -> Self {
        Self::Vector {
            message: message.into(),
            user_message: user_message.into(),
        }
    }

    pub fn claude(message: impl Into<String>, user_message: impl Into<String>) -> Self {
        Self::Claude {
            message: message.into(),
            user_message: user_message.into(),
        }
    }

    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::BadRequest(message.into())
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::NotFound(message.into())
    }

    pub fn unprocessable(message: impl Into<String>) -> Self {
        Self::Unprocessable(message.into())
    }

    pub fn internal(message: impl Into<String>, user_message: impl Into<String>) -> Self {
        Self::Internal {
            message: message.into(),
            user_message: user_message.into(),
        }
    }

    pub fn user_message(&self) -> &str {
        match self {
            Self::Document { user_message, .. }
            | Self::Vector { user_message, .. }
            | Self::Claude { user_message, .. }
            | Self::Internal { user_message, .. } => user_message,
            Self::BadRequest(message) | Self::NotFound(message) | Self::Unprocessable(message) => {
                message
            }
        }
    }

    fn status(&self) -> StatusCode {
        match self {
            Self::Document { .. } | Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::NotFound(_) => StatusCode::NOT_FOUND,
            Self::Unprocessable(_) => StatusCode::UNPROCESSABLE_ENTITY,
            Self::Vector { .. } | Self::Claude { .. } => StatusCode::SERVICE_UNAVAILABLE,
            Self::Internal { .. } => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = self.status();
        let detail = self.user_message().to_string();
        if status.is_server_error() {
            tracing::error!(error = %self, %status, "solicitud fallida");
        } else {
            tracing::warn!(error = %self, %status, "solicitud rechazada");
        }
        (status, Json(json!({ "detail": detail }))).into_response()
    }
}
