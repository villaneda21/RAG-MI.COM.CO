//! Backend de soporte de MI.COM.CO.
//!
//! La base de conocimiento, los casos y las conversaciones viven en PostgreSQL.
//! Las rutas delegan en servicios y los servicios en repositorios.

#![deny(clippy::unwrap_used)]

pub mod config;
pub mod db;
pub mod domain;
pub mod error;
pub mod repositories;
pub mod routes;
pub mod services;
pub mod state;

use std::error::Error;

use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;

use crate::config::Config;
use crate::error::StartupError;
use crate::services::index_service;
use crate::state::AppState;

pub async fn run() -> Result<(), Box<dyn Error>> {
    init_tracing();
    config::load_env();
    let config = Config::from_env()?;
    tracing::info!(
        host = %config.host,
        port = config.port,
        documents = %config.documents_dir.display(),
        "configuración cargada"
    );

    let pool = db::connect(&config.database_url).await.map_err(|err| {
        StartupError::new(
            "No se pudo conectar a PostgreSQL. Revisa DATABASE_URL y que el servicio esté en ejecución.",
        )
        .with_source_message(err)
    })?;
    db::apply_migrations(&pool).await.map_err(|err| {
        tracing::error!(error = %err, "migración fallida");
        StartupError::new(format!(
            "No se pudieron aplicar las migraciones de PostgreSQL: {err}"
        ))
    })?;

    let state = AppState::new(pool, config);
    index_service::index_if_empty(&state).await?;

    let address = format!("{}:{}", state.config.host, state.config.port);
    let listener = TcpListener::bind(&address).await?;
    tracing::info!("servidor listo en http://{address}");
    axum::serve(listener, routes::router(state))
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

fn init_tracing() {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,sqlx=warn"));
    let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
}

async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(err) = tokio::signal::ctrl_c().await {
            tracing::error!(error = %err, "no se pudo escuchar ctrl+c");
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(err) => tracing::error!(error = %err, "no se pudo escuchar SIGTERM"),
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }
    tracing::info!("apagado solicitado");
}
