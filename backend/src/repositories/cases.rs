use chrono::Utc;
use sqlx::PgPool;
use uuid::Uuid;

use crate::domain::cases::{new_case_id, ValidatedCase, CASE_STATUS};
use crate::domain::models::CaseRecord;
use crate::domain::sessions::is_safe_session_id;
use crate::domain::timeutil::format_bogota;
use crate::error::AppError;

pub struct CaseRepository<'a> {
    pool: &'a PgPool,
}

impl<'a> CaseRepository<'a> {
    pub fn new(pool: &'a PgPool) -> Self {
        Self { pool }
    }

    pub async fn register(
        &self,
        draft: &ValidatedCase,
        session_id: &str,
    ) -> Result<CaseRecord, AppError> {
        let session = session_id.trim();
        if !session.is_empty() && !is_safe_session_id(session) {
            return Err(AppError::BadRequest(
                "El identificador de sesión no es válido.".into(),
            ));
        }
        let now = Utc::now();
        let case_id = new_case_id(now);
        let summary = format!("{} · {}", draft.categoria, draft.correo);
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            r#"
            INSERT INTO support_cases (
                id, case_id, correo, categoria, detalle, registrado_en, estado, session_id
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(&case_id)
        .bind(&draft.correo)
        .bind(&draft.categoria)
        .bind(&draft.detalle)
        .bind(now)
        .bind(CASE_STATUS)
        .bind(session)
        .execute(&mut *tx)
        .await?;

        if !session.is_empty() {
            sqlx::query(
                r#"
                UPDATE conversations
                SET status = 'closed',
                    case_id = $2,
                    correo = $3,
                    categoria = $4,
                    updated_at = $5,
                    summary = $6
                WHERE session_id = $1 AND status <> 'closed'
                "#,
            )
            .bind(session)
            .bind(&case_id)
            .bind(&draft.correo)
            .bind(&draft.categoria)
            .bind(now)
            .bind(&summary)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        tracing::info!(case_id = %case_id, categoria = %draft.categoria, "caso registrado");
        Ok(CaseRecord {
            case_id,
            correo: draft.correo.clone(),
            categoria: draft.categoria.clone(),
            detalle: draft.detalle.clone(),
            registrado_en: format_bogota(now),
            estado: CASE_STATUS.to_string(),
            session_id: session.to_string(),
        })
    }

    pub async fn list(&self, limit: i64) -> Result<Vec<CaseRecord>, AppError> {
        let rows = sqlx::query_as::<_, CaseRow>(
            r#"
            SELECT case_id, correo, categoria, detalle, registrado_en, estado, session_id
            FROM support_cases
            ORDER BY registrado_en DESC
            LIMIT $1
            "#,
        )
        .bind(limit.clamp(1, 100))
        .fetch_all(self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|row| CaseRecord {
                case_id: row.case_id,
                correo: row.correo,
                categoria: row.categoria,
                detalle: row.detalle,
                registrado_en: format_bogota(row.registrado_en),
                estado: row.estado,
                session_id: row.session_id,
            })
            .collect())
    }
}

#[derive(sqlx::FromRow)]
struct CaseRow {
    case_id: String,
    correo: String,
    categoria: String,
    detalle: String,
    registrado_en: chrono::DateTime<Utc>,
    estado: String,
    session_id: String,
}
