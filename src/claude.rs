//! Cliente HTTP de la API Messages de Anthropic. Porta `app/services/claude_service.py`.

use std::time::Duration;

use serde_json::{json, Value};

use crate::cases::{CaseRecord, CaseService, CATEGORIES};
use crate::config::{is_usable_anthropic_key, Settings};
use crate::error::AppError;
use crate::models::ChatMessage;

const ANTHROPIC_URL: &str = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const MISSING_MODEL_HINT: &str = "El modelo de Claude configurado ya no está disponible (Anthropic lo retiró). En tu archivo .env pon CLAUDE_MODEL=claude-haiku-4-5 y reinicia el servidor.";

pub const SUPPORT_SYSTEM_PROMPT: &str = r#"Eres un compañero de soporte de MI.COM.CO. Estás conversando en tiempo real con un cliente.

Tu trabajo principal es orientar con la base de conocimiento (guías, plazos, políticas y pasos del panel). Registrar un caso es el último recurso, no el primero.

[REGLA DIRECTA E INVIOLABLE]
- Bajo ninguna circunstancia hagas diagnósticos en vivo ni verificaciones de conexiones: no revises DNS real, SPF/DKIM/DMARC, ping, IPs, ni el estado de red o servidores.
- Sí puedes explicar los procedimientos que aparecen en la base de conocimiento (por ejemplo cómo agregar un registro en la Zona DNS del panel), sin comprobar si ya quedó aplicado.
- No inventes pasos, precios, plazos ni políticas que no estén en los fragmentos recuperados.
- Si el cliente pide que "revises" o "verifiques" su DNS, SPF, ping o servidores, no lo hagas: entrega la guía documentada si existe y, si aún necesita gestión humana, ofrece registrar un caso.

[CÓMO USAR LA DOCUMENTACIÓN]
- Lee con cuidado los fragmentos recuperados y responde con esa información: pasos del panel, calendarios de vencimiento, auto-renovación, restauración, facturación, correo, hosting, etc.
- Si varios fragmentos aplican, combina lo relevante (por ejemplo renovar un dominio: avisos de vencimiento, qué pasa al expirar, auto-renovación en Seguridad, periodo de restauración).
- Si un fragmento no aplica a la pregunta, ignóralo.
- Si la base no cubre el punto, dilo con claridad y entonces ofrece dejar un caso.

[TONO Y ESTILO]
- Comunícate de forma natural, empática, fluida y amigable.
- Evita sonar como un contestador automático o un checklist rígido.
- Lenguaje cercano y profesional. Uno o dos emojis como máximo.
- Español siempre. No menciones que eres Claude ni detalles internos del sistema.

[FLUJO DE ATENCIÓN]
1. Orientación con la base (prioridad):
   - Si el cliente pregunta algo que está en la documentación, responde de inmediato con esa guía. No esperes el correo para orientar.
   - No registres un caso en el primer mensaje solo porque hay un problema. Primero entrega la información.
   - Después de orientar, pregunta si con eso pudo avanzar o si necesita que dejemos un caso para gestión de cuenta.

2. Apertura de caso (solo si hace falta):
   - Cuando la guía no alcance, el cliente pida explícitamente que lo dejen registrado, o se requiera una gestión de cuenta que el chat no puede ejecutar.
   - Pide el correo con el que está registrado en MI.COM.CO.
   - Clasifica únicamente en: Correo electrónico / Dominio / Hosting / Facturación o Compras.
   - Resume correo, categoría y detalle, y confirma con el cliente.
   - Solo cuando confirme, llama a la herramienta registrar_caso.
   - Tras el registro, despídete e incluye el número de caso si el sistema lo devolvió.

No registres el caso hasta que el cliente haya confirmado. No inventes un correo ni una categoría.
Cada conversación es independiente. Si vas a abrir un caso y en este chat todavía no hay correo, pídelo de nuevo aunque el cliente diga que ya lo dio en un caso anterior.
"#;

pub const RAG_SYSTEM_PROMPT: &str = "\
Eres el asistente virtual de MI.COM.CO.\n\
Respondes en español, de forma clara y profesional.\n\
Utiliza únicamente la información del contexto recuperado. No inventes datos.\n";

const INSUFFICIENT: &str =
    "No encontré información suficiente sobre esta pregunta en la base de conocimiento disponible.";

#[derive(Debug, Clone)]
pub struct SupportTurn {
    pub text: String,
    pub case: Option<CaseRecord>,
}

#[derive(Clone)]
pub struct ClaudeService {
    api_key: String,
    model: String,
    max_tokens: u32,
    temperature: f32,
    fallbacks: Vec<String>,
    http: reqwest::Client,
}

impl ClaudeService {
    pub fn new(settings: &Settings) -> Result<Self, AppError> {
        crate::tls::install_ring_provider();
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(90))
            .user_agent("micomco-rag/1.0")
            .build()
            .map_err(|err| {
                AppError::internal(
                    format!("No se pudo crear el cliente HTTP de Claude: {err}"),
                    "No se pudo preparar el cliente de Claude.",
                )
            })?;
        Ok(Self {
            api_key: settings.anthropic_api_key.clone(),
            model: settings.claude_model.clone(),
            max_tokens: settings.claude_max_tokens,
            temperature: settings.claude_temperature,
            fallbacks: settings.claude_fallback_models.clone(),
            http,
        })
    }

    pub fn is_configured(&self) -> bool {
        is_usable_anthropic_key(&self.api_key)
    }

    pub async fn generate_response(
        &self,
        question: &str,
        context: &str,
    ) -> Result<String, AppError> {
        let user_payload = format!(
            "INFORMACIÓN DE LA BASE DE CONOCIMIENTO:\n{}\n\nPREGUNTA DEL USUARIO:\n{}\n\nINSTRUCCIONES:\nResponde únicamente utilizando la información proporcionada.\nNo inventes información.\nSi la respuesta no puede obtenerse del contexto, indícalo claramente.",
            context.trim(),
            question.trim()
        );
        let content = self
            .create_message(
                vec![json!({"role": "user", "content": user_payload})],
                RAG_SYSTEM_PROMPT,
                None,
                self.temperature,
            )
            .await?;
        let answer = text_from(&content);
        if answer.is_empty() {
            Ok(INSUFFICIENT.to_string())
        } else {
            Ok(answer)
        }
    }

    pub async fn generate_support_reply(
        &self,
        question: &str,
        history: &[ChatMessage],
        session_id: &str,
        context: &str,
        cases: &std::sync::Mutex<CaseService>,
    ) -> Result<SupportTurn, AppError> {
        let messages = history_to_messages(history, question);
        let system = support_system_with_context(context);
        let temperature = if context.trim().is_empty() { 0.5 } else { 0.25 };
        let content = self
            .create_message(
                messages.clone(),
                &system,
                Some(register_case_tool()),
                temperature,
            )
            .await?;
        let tool = find_tool(&content);
        let spoken = text_from(&content);
        let Some(tool) = tool else {
            let text = if spoken.is_empty() {
                "Cuéntame un poco más para orientarte con las guías, o dejamos un caso si hace falta."
                    .to_string()
            } else {
                spoken
            };
            return Ok(SupportTurn { text, case: None });
        };

        let input = tool.get("input").cloned().unwrap_or_else(|| json!({}));
        let tool_id = tool
            .get("id")
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .to_string();
        let (record, tool_result) = {
            let store = cases.lock().map_err(|_| {
                AppError::internal(
                    "lock de casos envenenado",
                    "Ocurrió un error interno al generar la respuesta. Revisa la consola del servidor.",
                )
            })?;
            match store.register(
                &value_as_string(input.get("correo")),
                &value_as_string(input.get("categoria")),
                &value_as_string(input.get("detalle")),
                session_id,
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
                Err(err) => {
                    tracing::warn!(error = %err, "no se pudo registrar el caso");
                    let payload = json!({"ok": false, "error": err.user_message});
                    (None, payload.to_string())
                }
            }
        };

        let mut follow_messages = messages;
        follow_messages.push(json!({"role": "assistant", "content": content}));
        follow_messages.push(json!({
            "role": "user",
            "content": [{
                "type": "tool_result",
                "tool_use_id": tool_id,
                "content": tool_result,
            }]
        }));
        let follow = self
            .create_message(
                follow_messages,
                &system,
                Some(register_case_tool()),
                temperature,
            )
            .await?;
        let follow_text = text_from(&follow);
        let farewell = if follow_text.is_empty() {
            spoken
        } else {
            follow_text
        };
        if record.is_some() {
            return Ok(SupportTurn {
                text: farewell,
                case: record,
            });
        }
        let text = if farewell.is_empty() {
            "¿Me confirmas el correo y el motivo para dejar el caso listo?".to_string()
        } else {
            farewell
        };
        Ok(SupportTurn { text, case: None })
    }

    async fn create_message(
        &self,
        messages: Vec<Value>,
        system: &str,
        tools: Option<Value>,
        temperature: f32,
    ) -> Result<Vec<Value>, AppError> {
        self.require_key()?;
        let candidates = models_to_try(&self.model, &self.fallbacks);
        if candidates.is_empty() {
            return Err(AppError::claude(
                "No hay un modelo de Claude configurado.",
                MISSING_MODEL_HINT,
            ));
        }
        tracing::info!(model = %self.model, "consulta a Claude");
        let mut last_status: Option<(u16, String)> = None;
        for (index, model_name) in candidates.iter().enumerate() {
            let mut payload = json!({
                "model": model_name,
                "max_tokens": self.max_tokens,
                "temperature": temperature,
                "system": system,
                "messages": messages,
            });
            if let Some(tools) = &tools {
                payload["tools"] = tools.clone();
            }
            match self.post_message(&payload).await {
                Ok(content) => {
                    if model_name != &self.model {
                        tracing::warn!(
                            preferred = %self.model,
                            used = %model_name,
                            "el modelo preferido no está disponible"
                        );
                    }
                    tracing::info!("Claude respondió");
                    return Ok(content);
                }
                Err(CallError::Status { status, detail }) => {
                    tracing::warn!(status, model = %model_name, %detail, "Claude rechazó la consulta");
                    let can_fallback = index + 1 < candidates.len()
                        && (status == 404
                            || (status == 400 && detail.to_lowercase().contains("model")));
                    if can_fallback {
                        last_status = Some((status, detail));
                        continue;
                    }
                    return Err(map_status(status, &detail, model_name));
                }
                Err(CallError::Transport(err)) => {
                    tracing::error!(error = %err, "error de conexión con Claude");
                    return Err(AppError::claude(
                        format!("No se pudo conectar con la API de Claude: {err}"),
                        "No hay conexión con el servicio de Claude. Verifica tu red e inténtalo de nuevo.",
                    ));
                }
                Err(CallError::App(err)) => return Err(err),
            }
        }
        let (status, detail) = last_status.unwrap_or((0, String::new()));
        Err(AppError::claude(
            "Claude no devolvió una respuesta.",
            user_message_for_api_status(status, &detail, &self.model),
        ))
    }

    fn require_key(&self) -> Result<(), AppError> {
        if self.is_configured() {
            Ok(())
        } else {
            Err(AppError::claude(
                "ANTHROPIC_API_KEY no está definida o sigue siendo un placeholder.",
                "Falta configurar la clave de Claude. Edita el archivo .env, pon tu ANTHROPIC_API_KEY real y reinicia el servidor.",
            ))
        }
    }

    async fn post_message(&self, payload: &Value) -> Result<Vec<Value>, CallError> {
        let response = self
            .http
            .post(ANTHROPIC_URL)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("content-type", "application/json")
            .json(payload)
            .send()
            .await
            .map_err(CallError::Transport)?;
        let status = response.status();
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .to_string();
        let bytes = read_stream(response).await?;
        if !status.is_success() {
            let detail = error_detail(&String::from_utf8_lossy(&bytes));
            return Err(CallError::Status {
                status: status.as_u16(),
                detail,
            });
        }
        if content_type.contains("text/event-stream") {
            return parse_anthropic_sse(&bytes).map_err(CallError::App);
        }
        let value: Value = serde_json::from_slice(&bytes).map_err(|err| {
            CallError::App(AppError::claude(
                format!("Respuesta de Claude ilegible: {err}"),
                "Ocurrió un problema al generar la respuesta. Inténtalo de nuevo.",
            ))
        })?;
        value
            .get("content")
            .and_then(|content| content.as_array())
            .cloned()
            .ok_or_else(|| {
                CallError::App(AppError::claude(
                    "Claude no devolvió contenido.",
                    "Ocurrió un problema al generar la respuesta. Inténtalo de nuevo.",
                ))
            })
    }
}

enum CallError {
    Status { status: u16, detail: String },
    Transport(reqwest::Error),
    App(AppError),
}

pub fn models_to_try(preferred: &str, fallbacks: &[String]) -> Vec<String> {
    let mut ordered = Vec::new();
    let mut push = |name: &str| {
        let cleaned = name.trim();
        if !cleaned.is_empty() && !ordered.iter().any(|item: &String| item == cleaned) {
            ordered.push(cleaned.to_string());
        }
    };
    push(preferred);
    for name in fallbacks {
        push(name);
    }
    ordered
}

pub fn user_message_for_api_status(status_code: u16, detail: &str, _model: &str) -> String {
    let text = detail.to_lowercase();
    if matches!(status_code, 404 | 400)
        && (text.contains("model") || text.contains("not_found") || status_code == 404)
    {
        return MISSING_MODEL_HINT.to_string();
    }
    if matches!(status_code, 401 | 403)
        && (text.contains("credit")
            || text.contains("billing")
            || text.contains("quota")
            || text.contains("plan"))
    {
        return "Tu cuenta de Anthropic no tiene crédito o permiso para usar Claude. Revisa facturación en https://console.anthropic.com/".to_string();
    }
    if status_code == 403 {
        return "Anthropic rechazó la consulta (permiso o facturación). Revisa tu plan en https://console.anthropic.com/".to_string();
    }
    if status_code == 429 {
        return "El servicio de Claude está ocupado. Espera un momento e inténtalo de nuevo."
            .to_string();
    }
    if status_code == 529 {
        return "Claude está saturado en este momento. Inténtalo de nuevo en unos segundos."
            .to_string();
    }
    if status_code >= 500 {
        return "Claude no pudo procesar la consulta. Inténtalo de nuevo más tarde.".to_string();
    }
    "Claude no pudo procesar la consulta. Si el problema continúa, revisa CLAUDE_MODEL y tu crédito en la consola de Anthropic.".to_string()
}

fn map_status(status: u16, detail: &str, model: &str) -> AppError {
    let lowered = detail.to_lowercase();
    let billing = lowered.contains("credit")
        || lowered.contains("billing")
        || lowered.contains("quota")
        || lowered.contains("plan");
    if status == 401 && !billing {
        return AppError::claude(
            "Anthropic rechazó la autenticación.",
            "La clave de Claude no es válida. Revisa ANTHROPIC_API_KEY en tu archivo .env.",
        );
    }
    if status == 429 {
        return AppError::claude(
            "Rate limit de Anthropic.",
            "El servicio de Claude está ocupado. Espera un momento e inténtalo de nuevo.",
        );
    }
    AppError::claude(
        format!("Claude API status {status}: {detail}"),
        user_message_for_api_status(status, detail, model),
    )
}

fn support_system_with_context(context: &str) -> String {
    let recovered = context.trim();
    let knowledge = if recovered.is_empty() {
        "[INFORMACIÓN RECUPERADA DE LA BASE DE CONOCIMIENTO]\nNo se recuperaron fragmentos para esta consulta. No inventes procedimientos. Si el cliente necesita ayuda concreta, ofrece registrar un caso.".to_string()
    } else {
        format!(
            "[INFORMACIÓN RECUPERADA DE LA BASE DE CONOCIMIENTO]\nUsa estos fragmentos para orientar al cliente. Priorízalos sobre cualquier impulso de escalar.\n\n{recovered}"
        )
    };
    format!("{SUPPORT_SYSTEM_PROMPT}\n\n{knowledge}")
}

fn history_to_messages(history: &[ChatMessage], question: &str) -> Vec<Value> {
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

fn register_case_tool() -> Value {
    json!([{
        "name": "registrar_caso",
        "description": "Registra y cierra el caso en el historial del sistema. Úsalo SOLO cuando el cliente ya dio su correo, la categoría del servicio y el detalle de la solicitud, y confirmó que esos datos son correctos.",
        "input_schema": {
            "type": "object",
            "properties": {
                "correo": { "type": "string", "description": "Correo electrónico del cliente." },
                "categoria": {
                    "type": "string",
                    "enum": CATEGORIES,
                    "description": "Categoría principal de la solicitud."
                },
                "detalle": { "type": "string", "description": "Resumen claro del motivo de contacto." }
            },
            "required": ["correo", "categoria", "detalle"]
        }
    }])
}

fn text_from(content: &[Value]) -> String {
    content
        .iter()
        .filter_map(|block| {
            if block.get("type").and_then(|value| value.as_str()) == Some("text") {
                block
                    .get("text")
                    .and_then(|value| value.as_str())
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn find_tool(content: &[Value]) -> Option<&Value> {
    content.iter().find(|block| {
        block.get("type").and_then(|value| value.as_str()) == Some("tool_use")
            && block.get("name").and_then(|value| value.as_str()) == Some("registrar_caso")
    })
}

fn value_as_string(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    }
}

fn error_detail(body: &str) -> String {
    let Ok(value) = serde_json::from_str::<Value>(body) else {
        return body.to_string();
    };
    if let Some(error) = value.get("error") {
        if let Some(message) = error.get("message").and_then(|item| item.as_str()) {
            return message.to_string();
        }
        return error.to_string();
    }
    if let Some(message) = value.get("message").and_then(|item| item.as_str()) {
        return message.to_string();
    }
    body.to_string()
}

async fn read_stream(response: reqwest::Response) -> Result<Vec<u8>, CallError> {
    use futures_util::StreamExt;
    let mut stream = response.bytes_stream();
    let mut buffer = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(CallError::Transport)?;
        buffer.extend_from_slice(&chunk);
    }
    Ok(buffer)
}

pub fn parse_anthropic_sse(bytes: &[u8]) -> Result<Vec<Value>, AppError> {
    let text = String::from_utf8_lossy(bytes).replace("\r\n", "\n");
    let mut blocks: Vec<BlockAcc> = Vec::new();
    for event in text.split("\n\n") {
        let mut data = String::new();
        for line in event.lines() {
            if let Some(rest) = line.strip_prefix("data:") {
                if !data.is_empty() {
                    data.push('\n');
                }
                data.push_str(rest.trim_start());
            }
        }
        if data.is_empty() || data == "[DONE]" {
            continue;
        }
        let Ok(payload) = serde_json::from_str::<Value>(&data) else {
            continue;
        };
        let kind = payload
            .get("type")
            .and_then(|value| value.as_str())
            .unwrap_or("");
        match kind {
            "content_block_start" => {
                let index = payload
                    .get("index")
                    .and_then(|value| value.as_u64())
                    .unwrap_or(0) as usize;
                ensure_block(&mut blocks, index);
                if let Some(block) = payload.get("content_block") {
                    blocks[index].kind = block
                        .get("type")
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string();
                    blocks[index].text = block
                        .get("text")
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string();
                    blocks[index].id = block
                        .get("id")
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string();
                    blocks[index].name = block
                        .get("name")
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string();
                }
            }
            "content_block_delta" => {
                let index = payload
                    .get("index")
                    .and_then(|value| value.as_u64())
                    .unwrap_or(0) as usize;
                ensure_block(&mut blocks, index);
                let delta = payload.get("delta").cloned().unwrap_or(Value::Null);
                match delta.get("type").and_then(|value| value.as_str()) {
                    Some("text_delta") => {
                        if let Some(piece) = delta.get("text").and_then(|value| value.as_str()) {
                            blocks[index].text.push_str(piece);
                        }
                    }
                    Some("input_json_delta") => {
                        if let Some(piece) =
                            delta.get("partial_json").and_then(|value| value.as_str())
                        {
                            blocks[index].partial_json.push_str(piece);
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
    let content = blocks
        .into_iter()
        .filter_map(|block| match block.kind.as_str() {
            "text" => Some(json!({"type": "text", "text": block.text})),
            "tool_use" => {
                let input = serde_json::from_str::<Value>(&block.partial_json)
                    .unwrap_or_else(|_| json!({}));
                Some(json!({
                    "type": "tool_use",
                    "id": block.id,
                    "name": block.name,
                    "input": input,
                }))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    if content.is_empty() {
        return Err(AppError::claude(
            "El stream de Claude no trajo contenido.",
            "Ocurrió un problema al generar la respuesta. Inténtalo de nuevo.",
        ));
    }
    Ok(content)
}

struct BlockAcc {
    kind: String,
    text: String,
    id: String,
    name: String,
    partial_json: String,
}

fn ensure_block(blocks: &mut Vec<BlockAcc>, index: usize) {
    while blocks.len() <= index {
        blocks.push(BlockAcc {
            kind: String::new(),
            text: String::new(),
            id: String::new(),
            name: String::new(),
            partial_json: String::new(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::{
        models_to_try, parse_anthropic_sse, user_message_for_api_status, SUPPORT_SYSTEM_PROMPT,
    };

    #[test]
    fn fallback_models_do_not_repeat() {
        let models = models_to_try(
            "claude-haiku-4-5",
            &[
                "claude-haiku-4-5".to_string(),
                "claude-haiku-4-5-20251001".to_string(),
            ],
        );
        assert_eq!(
            models,
            vec![
                "claude-haiku-4-5".to_string(),
                "claude-haiku-4-5-20251001".to_string()
            ]
        );
    }

    #[test]
    fn status_messages_are_specific() {
        let missing = user_message_for_api_status(404, "model: not_found", "old");
        assert!(missing.contains("claude-haiku-4-5"));
        let busy = user_message_for_api_status(429, "", "claude-haiku-4-5");
        assert!(busy.contains("ocupado"));
    }

    #[test]
    fn system_prompt_covers_support_flow() {
        assert!(SUPPORT_SYSTEM_PROMPT.contains("DNS"));
        assert!(SUPPORT_SYSTEM_PROMPT.to_lowercase().contains("registrar"));
        assert!(SUPPORT_SYSTEM_PROMPT.contains("Facturación o Compras"));
        assert!(SUPPORT_SYSTEM_PROMPT
            .to_lowercase()
            .contains("base de conocimiento"));
        assert!(SUPPORT_SYSTEM_PROMPT
            .to_lowercase()
            .contains("último recurso"));
    }

    #[test]
    fn sse_assembles_text_and_tool_use() {
        let raw = "\
event: content_block_start
data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}

event: content_block_delta
data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hola\"}}

event: content_block_start
data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_1\",\"name\":\"registrar_caso\",\"input\":{}}}

event: content_block_delta
data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"correo\\\":\\\"ana@empresa.co\\\"}\"}}

event: message_stop
data: {\"type\":\"message_stop\"}
";
        let content = parse_anthropic_sse(raw.as_bytes()).unwrap();
        assert_eq!(content[0]["text"], "Hola");
        assert_eq!(content[1]["name"], "registrar_caso");
        assert_eq!(content[1]["input"]["correo"], "ana@empresa.co");
    }
}
