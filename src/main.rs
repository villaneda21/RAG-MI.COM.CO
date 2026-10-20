//! Arranque del asistente RAG. Sustituye a `python run.py`.

use std::net::SocketAddr;
use std::process::ExitCode;

use clap::Parser;
use tracing_subscriber::EnvFilter;

use rag_micomco::build_rag;
use rag_micomco::config::Settings;
use rag_micomco::rag::prepare_index;
use rag_micomco::routes::{router, AppState};

#[derive(Debug, Parser)]
#[command(
    name = "rag-micomco",
    about = "Crea el índice si hace falta y arranca el asistente RAG de MI.COM.CO."
)]
struct Args {
    /// Dirección de escucha. Por defecto usa HOST del entorno o 0.0.0.0.
    #[arg(long)]
    host: Option<String>,
    /// Puerto. Por defecto usa PORT del entorno o 8000.
    #[arg(long)]
    port: Option<u16>,
    /// No indexar ni precargar embeddings al arrancar.
    #[arg(long)]
    no_ingest: bool,
}

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse();
    let mut settings = Settings::load();
    if let Some(host) = args.host {
        settings.host = host;
    }
    if let Some(port) = args.port {
        settings.port = port;
    }

    tracing::info!("Inicio del sistema RAG MI.COM.CO");
    tracing::info!("Raíz del proyecto: {}", settings.project_root.display());
    tracing::info!("Modelo de embeddings: {}", settings.embedding_model);
    tracing::info!("Modelo de Claude: {}", settings.claude_model);
    tracing::info!(
        "Claude configurado: {}",
        if settings.claude_is_configured() {
            "sí"
        } else {
            "no (faltará para /api/chat)"
        }
    );

    let rag = match build_rag(settings) {
        Ok(rag) => rag,
        Err(err) => {
            eprintln!("[run] {err}");
            eprintln!("[run] {}", err.user_message);
            return ExitCode::from(1);
        }
    };

    if let Err(err) = rag.vector.connect().await {
        tracing::error!("ChromaDB no está disponible al iniciar. El health lo reportará. {err}");
    } else {
        tracing::info!(
            "ChromaDB listo. Chunks indexados: {}",
            rag.vector.count().await
        );
    }

    if !args.no_ingest {
        if let Err(err) = prepare_index(&rag).await {
            tracing::error!("No se pudo preparar la base vectorial: {err}");
            tracing::error!(
                "El servidor arrancará igual; usa /api/reindex o cargo run --bin ingest"
            );
        }
    }

    let state = match AppState::try_new(rag) {
        Ok(state) => state,
        Err(err) => {
            eprintln!("[run] {err}");
            return ExitCode::from(1);
        }
    };
    let host = state.rag.settings.host.clone();
    let port = state.rag.settings.port;
    let app = router(state);
    let addr: SocketAddr = match format!("{host}:{port}").parse() {
        Ok(addr) => addr,
        Err(err) => {
            eprintln!("[run] Dirección inválida {host}:{port}: {err}");
            return ExitCode::from(1);
        }
    };
    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(listener) => listener,
        Err(err) => {
            eprintln!("[run] No se pudo escuchar en {addr}: {err}");
            return ExitCode::from(1);
        }
    };
    let browse = if host == "0.0.0.0" || host == "::" {
        format!("http://127.0.0.1:{port}")
    } else {
        format!("http://{host}:{port}")
    };
    tracing::info!("Servidor listo en {browse}");
    if let Err(err) = axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
            tracing::info!("Sistema RAG detenido.");
        })
        .await
    {
        eprintln!("[run] El servidor se detuvo: {err}");
        return ExitCode::from(1);
    }
    ExitCode::SUCCESS
}
