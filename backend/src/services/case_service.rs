use crate::domain::cases::validate_case;
use crate::domain::models::{CaseResponse, CreateCaseRequest};
use crate::domain::sessions::is_safe_session_id;
use crate::error::AppError;
use crate::repositories::cases::CaseRepository;
use crate::state::AppState;

pub async fn create_case(
    state: &AppState,
    request: CreateCaseRequest,
) -> Result<CaseResponse, AppError> {
    let draft = validate_case(&request.correo, &request.categoria, &request.detalle)
        .map_err(AppError::BadRequest)?;
    let session = request.session_id.as_deref().unwrap_or("").trim();
    if !session.is_empty() && !is_safe_session_id(session) {
        return Err(AppError::BadRequest(
            "El identificador de sesión no es válido.".into(),
        ));
    }
    let record = CaseRepository::new(&state.pool)
        .register(&draft, session)
        .await?;
    Ok(record.to_response())
}
