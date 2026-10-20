//! Cliente HTTP de la API oficial de Anthropic (Claude).

use std::time::Duration;

use async_trait::async_trait;
use futures_util::StreamExt;
use reqwest::header::CONTENT_TYPE;
use reqwest::Client;
use serde_json::{json, Value};

use crate::cases::{CaseRecord, CaseRegistrationError, CaseService, CATEGORIES};
use crate::config::{is_usable_anthropic_key, Settings, CLAUDE_FALLBACK_MODELS};
use crate::conversations::HistoryTurn;
use crate::prompts::{
    support_system_with_context, INSUFFICIENT_INFO_FALLBACK, MISSING_MODEL_HINT, RAG_SYSTEM_PROMPT,
};

const MAX_BODY: usize = 8 * 1024 * 1024;
const ANTHROPIC_URL: &str = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_VERSION: &str = "2023-06-01";

#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct ClaudeServiceError {
    pub message: String,
    pub user_message: String,
}

impl ClaudeServiceError {
    pub fn new(message: impl Into<String>, user_message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            user_message: user_message.into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct SupportTurn {
    pub text: String,
    pub case: Option<CaseRecord>,
}

#[async_trait]
pub trait SupportClient: Send + Sync {
    fn is_configured(&self) -> bool;
    async fn generate_support_reply(
        &self,
        question: String,
        history: Vec<HistoryTurn>,
        session_id: String,
        case_service: CaseService,
        context: String,
    ) -> Result<SupportTurn, ClaudeServiceError>;
}

#[derive(Clone)]
pub struct ClaudeService {
    http: Client,
    api_key: String,
    model: String,
    max_tokens: u32,
    temperature: f32,
    fallbacks: Vec<String>,
}

impl ClaudeService {
    pub fn from_settings(settings: &Settings) -> Self {
        Self::new(
            settings.anthropic_api_key.clone(),
            settings.claude_model.clone(),
            settings.claude_max_tokens,
            settings.claude_temperature,
            CLAUDE_FALLBACK_MODELS
                .iter()
                .map(|model| (*model).to_string())
                .collect(),
        )
    }

    pub fn new(
        api_key: String,
        model: String,
        max_tokens: u32,
        temperature: f32,
        fallbacks: Vec<String>,
    ) -> Self {
        let http = Client::builder()
            .timeout(Duration::from_secs(90))
            .build()
            .unwrap_or_else(|_| Client::new());
        Self {
            http,
            api_key: api_key.trim().to_string(),
            model,
            max_tokens,
            temperature,
            fallbacks,
        }
    }

    pub fn is_configured(&self) -> bool {
        is_usable_anthropic_key(Some(&self.api_key))
    }

    fn require_key(&self) -> Result<(), ClaudeServiceError> {
        if self.is_configured() {
            return Ok(());
        }
        Err(ClaudeServiceError::new(
            "ANTHROPIC_API_KEY no está definida o sigue siendo un placeholder.",
            "Falta configurar la clave de Claude. Edita el archivo .env, pon tu ANTHROPIC_API_KEY real y reinicia el servidor.",
        ))
    }

    pub async fn generate_response(
        &self,
        question: &str,
        context: &str,
    ) -> Result<String, ClaudeServiceError> {
        let user_payload = format!(
            "INFORMACIÓN DE LA BASE DE CONOCIMIENTO:\n{}\n\nPREGUNTA DEL USUARIO:\n{}\n\nINSTRUCCIONES:\nResponde únicamente utilizando la información proporcionada.\nNo inventes información.\nSi la respuesta no puede obtenerse del contexto, indícalo claramente.",
            context.trim(),
            question.trim()
        );
        let message = self
            .create_message(
                vec![json!({"role": "user", "content": user_payload})],
                RAG_SYSTEM_PROMPT,
                None,
                Some(self.temperature),
            )
            .await?;
        let answer = text_from_content(message.get("content").and_then(|value| value.as_array()));
        if answer.is_empty() {
            Ok(INSUFFICIENT_INFO_FALLBACK.to_string())
        } else {
            Ok(answer)
        }
    }

    async fn create_message(
        &self,
        messages: Vec<Value>,
        system: &str,
        tools: Option<Value>,
        temperature: Option<f32>,
    ) -> Result<Value, ClaudeServiceError> {
        self.require_key()?;
        let candidates = models_to_try(
            &self.model,
            &self
                .fallbacks
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
        );
        tracing::info!("Consulta a Claude (preferido={}).", self.model);
        let mut last_status: Option<u16> = None;
        let mut last_detail = String::new();
        for (index, model_name) in candidates.iter().enumerate() {
            let mut payload = json!({
                "model": model_name,
                "max_tokens": self.max_tokens,
                "temperature": temperature.unwrap_or(self.temperature),
                "system": system,
                "messages": messages,
            });
            if let Some(tools) = &tools {
                payload["tools"] = tools.clone();
            }
            let response = match self
                .http
                .post(ANTHROPIC_URL)
                .header("x-api-key", &self.api_key)
                .header("anthropic-version", ANTHROPIC_VERSION)
                .header(CONTENT_TYPE, "application/json")
                .json(&payload)
                .send()
                .await
            {
                Ok(response) => response,
                Err(err) => {
                    tracing::error!("Error de conexión con Claude: {err}");
                    return Err(ClaudeServiceError::new(
                        format!("No se pudo conectar con la API de Claude: {err}"),
                        "No hay conexión con el servicio de Claude. Verifica tu red e inténtalo de nuevo.",
                    ));
                }
            };
            let status = response.status();
            let body = read_json_stream(response).await?;
            if status.is_success() {
                if model_name != &self.model {
                    tracing::warn!(
                        "El modelo {} no está disponible. Se usó {model_name}.",
                        self.model
                    );
                }
                tracing::info!("Claude respondió.");
                return Ok(body);
            }
            let detail = error_detail(&body);
            tracing::warn!("Claude HTTP {status} con modelo {model_name}: {detail}");
            let can_fallback = index + 1 < candidates.len()
                && (status.as_u16() == 404
                    || (status.as_u16() == 400 && detail.to_lowercase().contains("model")));
            if can_fallback {
                last_status = Some(status.as_u16());
                last_detail = detail;
                continue;
            }
            return Err(ClaudeServiceError::new(
                format!("Claude API status {status}: {detail}"),
                user_message_for_api_status(Some(status.as_u16()), &detail, model_name),
            ));
        }
        Err(ClaudeServiceError::new(
            "Claude no devolvió una respuesta.",
            user_message_for_api_status(last_status, &last_detail, &self.model),
        ))
    }
}

#[async_trait]
impl SupportClient for ClaudeService {
    fn is_configured(&self) -> bool {
        ClaudeService::is_configured(self)
    }

    async fn generate_support_reply(
        &self,
        question: String,
        history: Vec<HistoryTurn>,
        session_id: String,
        case_service: CaseService,
        context: String,
    ) -> Result<SupportTurn, ClaudeServiceError> {
        let messages = history_to_messages(&history, &question);
        let system = support_system_with_context(&context);
        let temperature = if context.trim().is_empty() { 0.5 } else { 0.25 };
        let tools = register_case_tool();
        let message = self
            .create_message(
                messages.clone(),
                &system,
                Some(tools.clone()),
                Some(temperature),
            )
            .await?;
        let content = message.get("content").and_then(|value| value.as_array());
        let spoken = text_from_content(content);
        let Some(tool_block) = content.and_then(|blocks| tool_use_block(blocks)) else {
            return Ok(SupportTurn {
                text: if spoken.is_empty() {
                    "Cuéntame un poco más para orientarte con las guías, o dejamos un caso si hace falta.".into()
                } else {
                    spoken
                },
                case: None,
            });
        };
        let Some(tool_id) = tool_block.get("id").and_then(|value| value.as_str()) else {
            tracing::warn!("Claude pidió registrar_caso sin id de herramienta.");
            return Ok(SupportTurn {
                text: spoken,
                case: None,
            });
        };
        let tool_input = tool_block
            .get("input")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let (record, tool_result) = match case_service.register(
            tool_input
                .get("correo")
                .and_then(|value| value.as_str())
                .unwrap_or(""),
            tool_input
                .get("categoria")
                .and_then(|value| value.as_str())
                .unwrap_or(""),
            tool_input
                .get("detalle")
                .and_then(|value| value.as_str())
                .unwrap_or(""),
            &session_id,
        ) {
            Ok(record) => {
                let payload = json!({
                    "ok": true,
                    "case_id": record.case_id,
                    "estado": record.estado,
                    "registrado_en": record.registrado_en,
                });
                (Some(record), payload.to_string())
            }
            Err(CaseRegistrationError { user_message, .. }) => {
                tracing::warn!("No se pudo registrar el caso: {user_message}");
                (
                    None,
                    json!({"ok": false, "error": user_message}).to_string(),
                )
            }
        };
        let mut follow_up = messages;
        follow_up.push(json!({
            "role": "assistant",
            "content": message.get("content").cloned().unwrap_or_else(|| json!([]))
        }));
        follow_up.push(json!({
            "role": "user",
            "content": [{
                "type": "tool_result",
                "tool_use_id": tool_id,
                "content": tool_result
            }]
        }));
        let follow = self
            .create_message(follow_up, &system, Some(tools), Some(temperature))
            .await?;
        let farewell = text_from_content(follow.get("content").and_then(|value| value.as_array()));
        let text = if farewell.is_empty() {
            spoken
        } else {
            farewell
        };
        if record.is_some() {
            Ok(SupportTurn { text, case: record })
        } else if text.is_empty() {
            Ok(SupportTurn {
                text: "¿Me confirmas el correo y el motivo para dejar el caso listo?".into(),
                case: None,
            })
        } else {
            Ok(SupportTurn { text, case: None })
        }
    }
}

pub fn models_to_try(preferred: &str, fallbacks: &[&str]) -> Vec<String> {
    let mut ordered = Vec::new();
    for name in std::iter::once(preferred).chain(fallbacks.iter().copied()) {
        let cleaned = name.trim();
        if !cleaned.is_empty() && !ordered.iter().any(|existing: &String| existing == cleaned) {
            ordered.push(cleaned.to_string());
        }
    }
    ordered
}

pub fn user_message_for_api_status(status_code: Option<u16>, detail: &str, _model: &str) -> String {
    let text = detail.to_lowercase();
    let code = status_code.unwrap_or(0);
    if matches!(code, 404 | 400)
        && (text.contains("model") || text.contains("not_found") || code == 404)
    {
        return MISSING_MODEL_HINT.to_string();
    }
    if matches!(code, 401 | 403)
        && (text.contains("credit")
            || text.contains("billing")
            || text.contains("quota")
            || text.contains("plan"))
    {
        return "Tu cuenta de Anthropic no tiene crédito o permiso para usar Claude. Revisa facturación en https://console.anthropic.com/".into();
    }
    if code == 403 {
        return "Anthropic rechazó la consulta (permiso o facturación). Revisa tu plan en https://console.anthropic.com/".into();
    }
    if code == 429 {
        return "El servicio de Claude está ocupado. Espera un momento e inténtalo de nuevo."
            .into();
    }
    if code == 529 {
        return "Claude está saturado en este momento. Inténtalo de nuevo en unos segundos.".into();
    }
    if code >= 500 {
        return "Claude no pudo procesar la consulta. Inténtalo de nuevo más tarde.".into();
    }
    "Claude no pudo procesar la consulta. Si el problema continúa, revisa CLAUDE_MODEL y tu crédito en la consola de Anthropic.".into()
}

fn register_case_tool() -> Value {
    json!([{
        "name": "registrar_caso",
        "description": "Registra y cierra el caso en el historial del sistema. Úsalo SOLO cuando el cliente ya dio su correo, la categoría del servicio y el detalle de la solicitud, y confirmó que esos datos son correctos.",
        "input_schema": {
            "type": "object",
            "properties": {
                "correo": {
                    "type": "string",
                    "description": "Correo electrónico del cliente."
                },
                "categoria": {
                    "type": "string",
                    "enum": CATEGORIES,
                    "description": "Categoría principal de la solicitud."
                },
                "detalle": {
                    "type": "string",
                    "description": "Resumen claro del motivo de contacto."
                }
            },
            "required": ["correo", "categoria", "detalle"]
        }
    }])
}

fn history_to_messages(history: &[HistoryTurn], question: &str) -> Vec<Value> {
    let mut messages = Vec::new();
    for item in history {
        let role = item.role.trim();
        let content = item.content.trim();
        if (role != "user" && role != "assistant") || content.is_empty() {
            continue;
        }
        messages.push(json!({"role": role, "content": content}));
    }
    messages.push(json!({"role": "user", "content": question.trim()}));
    messages
}

fn text_from_content(content: Option<&Vec<Value>>) -> String {
    let Some(blocks) = content else {
        return String::new();
    };
    let mut parts = Vec::new();
    for block in blocks {
        if let Some(text) = block.get("text").and_then(|value| value.as_str()) {
            if !text.is_empty() {
                parts.push(text);
            }
        }
    }
    parts.join("\n").trim().to_string()
}

fn tool_use_block(content: &[Value]) -> Option<&Value> {
    content.iter().find(|block| {
        block.get("type").and_then(|value| value.as_str()) == Some("tool_use")
            && block.get("name").and_then(|value| value.as_str()) == Some("registrar_caso")
    })
}

fn error_detail(body: &Value) -> String {
    if let Some(error) = body.get("error") {
        if let Some(object) = error.as_object() {
            if let Some(message) = object.get("message").and_then(|value| value.as_str()) {
                return message.to_string();
            }
            return error.to_string();
        }
        return error.to_string();
    }
    if let Some(message) = body.get("message").and_then(|value| value.as_str()) {
        return message.to_string();
    }
    if body.is_null() {
        return String::new();
    }
    body.to_string()
}

async fn read_json_stream(response: reqwest::Response) -> Result<Value, ClaudeServiceError> {
    let mut stream = response.bytes_stream();
    let mut buffer = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|err| {
            ClaudeServiceError::new(
                format!("Lectura incompleta de Claude: {err}"),
                "Ocurrió un problema al generar la respuesta. Inténtalo de nuevo.",
            )
        })?;
        if buffer.len().saturating_add(chunk.len()) > MAX_BODY {
            return Err(ClaudeServiceError::new(
                "La respuesta de Claude supera el tamaño permitido.",
                "Ocurrió un problema al generar la respuesta. Inténtalo de nuevo.",
            ));
        }
        buffer.extend_from_slice(&chunk);
    }
    if buffer.is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_slice(&buffer).map_err(|err| {
        ClaudeServiceError::new(
            format!("JSON inválido de Claude: {err}"),
            "Ocurrió un problema al generar la respuesta. Inténtalo de nuevo.",
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prompts::SUPPORT_SYSTEM_PROMPT;

    #[test]
    fn retired_model_gets_clear_user_message() {
        let message = user_message_for_api_status(
            Some(404),
            "model: claude-sonnet-4-20250514",
            "claude-sonnet-4-20250514",
        );
        assert!(message.contains("claude-haiku-4-5"));
        assert!(message.contains("retiró") || message.contains("retirado"));
        assert_eq!(
            models_to_try("claude-sonnet-4-20250514", &["claude-haiku-4-5"]),
            vec![
                "claude-sonnet-4-20250514".to_string(),
                "claude-haiku-4-5".to_string()
            ]
        );
    }

    #[test]
    fn system_prompt_covers_support_flow() {
        let lowered = SUPPORT_SYSTEM_PROMPT.to_lowercase();
        assert!(SUPPORT_SYSTEM_PROMPT.contains("DNS"));
        assert!(lowered.contains("registrar"));
        assert!(SUPPORT_SYSTEM_PROMPT.contains("Facturación o Compras"));
        assert!(lowered.contains("base de conocimiento"));
        assert!(lowered.contains("último recurso") || lowered.contains("no el primero"));
    }
}
