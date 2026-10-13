use crate::domain::answer::{
    build_retrieval_query, classify_turn, farewell, render_knowledge, sanitize_history,
    select_hits, validate_question, Classification,
};
use crate::domain::models::{ChatRequest, ChatResponse};
use crate::error::AppError;
use crate::repositories::cases::CaseRepository;
use crate::repositories::conversations::{ConversationRepository, NewTurn};
use crate::repositories::knowledge::KnowledgeRepository;
use crate::state::AppState;

pub async fn ask(state: &AppState, request: ChatRequest) -> Result<ChatResponse, AppError> {
    let question = validate_question(&request.question).map_err(AppError::BadRequest)?;
    let history = sanitize_history(&request.history);
    let conversations = ConversationRepository::new(&state.pool);
    let (session_id, new_chat) = conversations.resolve(request.session_id.as_deref()).await?;
    let effective = if new_chat { Vec::new() } else { history };
    let classification = classify_turn(&question, &effective);

    let (answer, sources, case_record) = match classification {
        Classification::Knowledge {
            diagnostic,
            offer_case,
        } => {
            let query = build_retrieval_query(&question, &effective);
            let hits = KnowledgeRepository::new(&state.pool)
                .search(&query, None, state.config.retrieval_k)
                .await?;
            let selected = select_hits(&hits);
            tracing::info!(
                session_id = %session_id,
                hits = selected.len(),
                chars = question.chars().count(),
                "consulta de conocimiento"
            );
            let sources = selected.iter().map(|hit| hit.to_source()).collect();
            let answer = render_knowledge(diagnostic, offer_case, &selected);
            (answer, sources, None)
        }
        Classification::Register(draft) => {
            let record = CaseRepository::new(&state.pool)
                .register(&draft, &session_id)
                .await?;
            let answer = farewell(
                &record.case_id,
                &record.estado,
                &record.correo,
                &record.categoria,
                &record.detalle,
            );
            (answer, Vec::new(), Some(record))
        }
        Classification::Ask(text) => (text, Vec::new(), None),
    };

    conversations
        .record_turn(NewTurn {
            session_id: &session_id,
            question: &question,
            answer: &answer,
            history: &effective,
            case: case_record.as_ref(),
        })
        .await?;

    Ok(ChatResponse {
        answer,
        sources,
        session_id: Some(session_id),
        case_closed: case_record.is_some(),
        case: case_record.as_ref().map(|record| record.to_summary()),
        new_chat,
    })
}
