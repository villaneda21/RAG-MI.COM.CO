//! Turno del cliente: enrutado, RAG sin fuentes y cierre con confirmación.

use uuid::Uuid;

use crate::conversations::HistoryTurn;
use crate::rag::RagService;

use super::logic::{
    client_confirms_resolution, requests_human, with_satisfaction_question, SATISFACTION_QUESTION,
};
use super::store::{Helpdesk, HelpdeskError, Message, TicketRow};

#[derive(Debug, Clone, serde::Serialize)]
pub struct ClientView {
    pub ticket: TicketRow,
    pub messages: Vec<Message>,
}

pub async fn triage(
    helpdesk: &Helpdesk,
    rag: &RagService,
    name: &str,
    domain: &str,
    email: &str,
    reason: &str,
) -> Result<ClientView, HelpdeskError> {
    let (_customer, ticket) = helpdesk.open_ticket(name, domain, email, reason).await?;
    reply_as_bot(helpdesk, rag, ticket.id).await
}

pub async fn accept_client_message(
    helpdesk: &Helpdesk,
    rag: &RagService,
    ticket_id: Uuid,
    content: &str,
    media_url: Option<&str>,
) -> Result<ClientView, HelpdeskError> {
    let ticket = helpdesk.ticket_by_id(ticket_id).await?;
    let media_type = if media_url.is_some() { "audio" } else { "text" };
    if ticket.status == "resuelto" || ticket.status == "cerrado" {
        let opened = helpdesk
            .open_followup(&ticket, content, media_type, media_url)
            .await?;
        return reply_as_bot(helpdesk, rag, opened).await;
    }
    helpdesk
        .record_client_message(
            ticket_id,
            content,
            media_type,
            media_url,
            &ticket.customer_name,
        )
        .await?;
    let ticket = helpdesk.ticket_by_id(ticket_id).await?;
    if ticket.status != "en_bot" && ticket.status != "nuevo" {
        return client_view(helpdesk, ticket_id).await;
    }
    reply_as_bot(helpdesk, rag, ticket_id).await
}

pub async fn client_view(
    helpdesk: &Helpdesk,
    ticket_id: Uuid,
) -> Result<ClientView, HelpdeskError> {
    let ticket = helpdesk.ticket_by_id(ticket_id).await?;
    let messages = helpdesk.messages(ticket_id, true).await?;
    Ok(ClientView { ticket, messages })
}

async fn reply_as_bot(
    helpdesk: &Helpdesk,
    rag: &RagService,
    ticket_id: Uuid,
) -> Result<ClientView, HelpdeskError> {
    let ticket = helpdesk.ticket_by_id(ticket_id).await?;
    if ticket.status != "en_bot" && ticket.status != "nuevo" {
        return client_view(helpdesk, ticket_id).await;
    }
    let messages = helpdesk.messages(ticket_id, true).await?;
    let Some(latest) = messages
        .iter()
        .rev()
        .find(|message| message.sender_type == "client")
    else {
        return client_view(helpdesk, ticket_id).await;
    };
    let question = latest.content.trim().to_string();
    if requests_human(&question) {
        let text = format!(
            "Te comunico con un agente de {}. En un momento continúan por este chat.",
            ticket.department_name
        );
        helpdesk
            .post_bot_message(
                ticket_id,
                &text,
                Some("esperando_agente"),
                "status_change",
                "El cliente pidió hablar con una persona.",
            )
            .await?;
        return client_view(helpdesk, ticket_id).await;
    }
    let prior: Vec<&super::store::Message> = messages
        .iter()
        .filter(|message| message.id != latest.id)
        .collect();
    let pending = prior
        .iter()
        .rev()
        .find(|message| message.sender_type == "bot")
        .is_some_and(|message| message.content.contains(SATISFACTION_QUESTION));
    if pending && client_confirms_resolution(&question) {
        helpdesk
            .post_bot_message(
                ticket_id,
                "Me alegra haber podido ayudarte. Cierro este caso; cuando quieras, escríbenos de nuevo.",
                Some("cerrado"),
                "bot_validation",
                "El cliente confirmó que la duda quedó resuelta.",
            )
            .await?;
        return client_view(helpdesk, ticket_id).await;
    }
    let answer = if question == "Nota de voz" {
        "Recibimos tu nota de voz y quedó disponible en este chat.".to_string()
    } else {
        let history = prior
            .iter()
            .filter_map(|message| {
                let role = match message.sender_type.as_str() {
                    "client" => "user",
                    "bot" | "agent" => "assistant",
                    _ => return None,
                };
                let content = message.content.trim();
                if content.is_empty() {
                    None
                } else {
                    Some(HistoryTurn {
                        role: role.to_string(),
                        content: content.to_string(),
                    })
                }
            })
            .collect::<Vec<_>>();
        match rag
            .ask(&question, &history, Some(&ticket_id.to_string()))
            .await
        {
            Ok(response) => response.answer,
            Err(err) => {
                tracing::warn!("El asistente no pudo responder en {ticket_id}: {err}");
                let text = format!(
                    "Recibí tu mensaje. Ahora mismo no puedo ampliar la respuesta automática, así que te dejo con un agente de {}.",
                    ticket.department_name
                );
                helpdesk
                    .post_bot_message(
                        ticket_id,
                        &text,
                        Some("esperando_agente"),
                        "status_change",
                        "Escalado porque el asistente no pudo responder.",
                    )
                    .await?;
                return client_view(helpdesk, ticket_id).await;
            }
        }
    };
    let visible = with_satisfaction_question(&answer);
    helpdesk
        .post_bot_message(
            ticket_id,
            &visible,
            Some("en_bot"),
            "bot_validation",
            "Respuesta automática enviada. Se pidió confirmación antes de cerrar.",
        )
        .await?;
    client_view(helpdesk, ticket_id).await
}
