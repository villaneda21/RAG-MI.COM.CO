//! Temporizador de inactividad. Corre cada minuto mientras el proceso vive.

use std::time::Duration;

use sqlx::PgPool;

use super::store::Helpdesk;

pub fn spawn_inactivity_worker(pool: PgPool, media_dir: std::path::PathBuf) {
    tokio::spawn(async move {
        let helpdesk = Helpdesk { pool, media_dir };
        let mut ticker = tokio::time::interval(Duration::from_secs(60));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            if let Err(err) = run_inactivity_pass(&helpdesk).await {
                tracing::error!("Temporizador de inactividad: {err}");
            }
        }
    });
}

pub async fn run_inactivity_pass(helpdesk: &Helpdesk) -> Result<(), super::store::HelpdeskError> {
    let rules = helpdesk.rules().await?;
    if !rules.auto_close_enabled {
        tracing::debug!("Cierre automático global desactivado.");
        return Ok(());
    }
    // El cierre se evalúa antes de la advertencia para no cerrar en el mismo ciclo
    // un chat que acaba de recibir el aviso.
    let closings = helpdesk
        .tickets_needing_close(rules.inactivity_close_minutes)
        .await?;
    for ticket_id in closings {
        if helpdesk
            .mark_closed_for_inactivity(
                ticket_id,
                &rules.close_message_template,
                rules.inactivity_close_minutes,
            )
            .await?
        {
            tracing::info!("Chat {ticket_id} cerrado por inactividad");
        }
    }
    let warnings = helpdesk
        .tickets_needing_warning(rules.inactivity_warning_minutes)
        .await?;
    for ticket_id in warnings {
        if helpdesk
            .mark_warning(ticket_id, &rules.warning_message_template)
            .await?
        {
            tracing::info!("Advertencia de inactividad enviada en {ticket_id}");
        }
    }
    Ok(())
}
