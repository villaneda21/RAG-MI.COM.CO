//! Arranque del asistente RAG. Sustituye a `run.py`.
//!
//! ```text
//! cargo run
//! cargo run -- --port 8000 --no-ingest
//! cargo run -- ingest --status
//! cargo run -- ingest --reset
//! cargo run -- ingest --file guia.txt
//! ```

use std::path::Path;

use clap::{Parser, Subcommand};
use micomco_rag::config::{ensure_env_file, load_dotenv, project_root, Settings};
use micomco_rag::error::AppError;
use micomco_rag::routes::router;
use micomco_rag::state::AppState;

#[derive(Debug, Parser)]
#[command(
    name = "micomco-rag",
    version,
    about = "Crea el índice de conocimiento y arranca el asistente RAG de MI.COM.CO."
)]
struct Args {
    /// Compatibilidad con run.py. El binario ya está compilado.
    #[arg(long)]
    skip_setup: bool,
    /// Compatibilidad con run.py. Este servidor no recarga el proceso.
    #[arg(long)]
    reload: bool,
    /// No indexar documentos al arrancar.
    #[arg(long)]
    no_ingest: bool,
    /// Host de escucha. Por defecto la variable HOST o 0.0.0.0.
    #[arg(long)]
    host: Option<String>,
    /// Puerto de escucha. Por defecto la variable PORT o 8000.
    #[arg(long)]
    port: Option<u16>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Indexa `data/documents/` (equivalente a `scripts/ingest_document.py`).
    Ingest {
        /// Elimina la colección y vuelve a indexar todos los archivos.
        #[arg(long)]
        reset: bool,
        /// Reindexa aunque el archivo no haya cambiado.
        #[arg(long)]
        force: bool,
        /// Muestra el estado de cada archivo y sale.
        #[arg(long)]
        status: bool,
        /// Indexa solo este archivo (nombre o ruta).
        #[arg(long)]
        file: Option<String>,
    },
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with_target(true)
        .init();

    let code = match run().await {
        Ok(code) => code,
        Err(err) => {
            eprintln!("[run] {}", err.user_message());
            eprintln!("[run] {err}");
            1
        }
    };
    if code != 0 {
        std::process::exit(code);
    }
}

async fn run() -> Result<i32, AppError> {
    let root = project_root();
    ensure_env_file(&root);
    load_dotenv(&root);
    let args = Args::parse();
    if args.reload {
        tracing::warn!("--reload no aplica al servidor Rust; se ignora");
    }
    let _ = args.skip_setup;

    let mut settings = Settings::load();
    settings.apply_bind(args.host.clone(), args.port);
    let state = AppState::init(settings).await?;
    log_startup(&state);

    match args.command {
        Some(Command::Ingest {
            reset,
            force,
            status,
            file,
        }) => ingest(state, reset, force, status, file).await,
        None => {
            if !args.no_ingest {
                ensure_index(&state).await;
            }
            serve(state).await?;
            Ok(0)
        }
    }
}

fn log_startup(state: &AppState) {
    tracing::info!("Inicio del sistema RAG MI.COM.CO");
    tracing::info!(root = %state.settings.project_root.display(), "raíz del proyecto");
    tracing::info!(model = %state.settings.embedding_model, "modelo de embeddings configurado");
    tracing::info!(
        dimensions = state.settings.embedding_dimensions,
        backend = if state.settings.uses_remote_chroma() {
            "chroma-http"
        } else {
            "indice-local"
        },
        "embeddings: hashing vectorizer + BM25"
    );
    tracing::info!(model = %state.settings.claude_model, "modelo de Claude");
    tracing::info!(
        configured = state.settings.claude_is_configured(),
        "Claude configurado"
    );
}

async fn ensure_index(state: &AppState) {
    let count = state.vectors.lock().await.count().await;
    let status = match state.index_status().await {
        Ok(status) => status,
        Err(err) => {
            println!(
                "[run] No se pudo preparar la base vectorial: {}",
                err.user_message()
            );
            println!("[run] El servidor arrancará igual; usa /api/reindex o cargo run -- ingest");
            return;
        }
    };
    if count == 0 || status.pending_changes {
        println!("[run] Indexando archivos nuevos o modificados en data/documents/ ...");
        match state.reindex(count == 0, false, None).await {
            Ok(result) => println!("[run] {}", result.message),
            Err(err) => {
                println!(
                    "[run] No se pudo preparar la base vectorial: {}",
                    err.user_message()
                );
                println!(
                    "[run] El servidor arrancará igual; usa /api/reindex o cargo run -- ingest"
                );
            }
        }
    } else {
        println!("[run] La base vectorial ya tiene {count} fragmentos al día.");
    }
    println!(
        "[run] Embeddings listos ({} dimensiones).",
        state.settings.embedding_dimensions
    );
}

async fn serve(state: std::sync::Arc<AppState>) -> Result<(), AppError> {
    let address = format!("{}:{}", state.settings.host, state.settings.port);
    let listener = tokio::net::TcpListener::bind(&address)
        .await
        .map_err(|err| {
            AppError::internal(
                format!("No se pudo escuchar en {address}: {err}"),
                format!("No se pudo abrir el puerto {address}."),
            )
        })?;
    println!(
        "[run] Servidor listo en http://127.0.0.1:{}",
        state.settings.port
    );
    if state.settings.host != "127.0.0.1" && state.settings.host != "localhost" {
        println!("[run] Escuchando en http://{address}");
    }
    let port = state.settings.port;
    axum::serve(listener, router(state))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
            println!("[run] Sistema RAG detenido.");
        })
        .await
        .map_err(|err| {
            AppError::internal(
                format!("El servidor se detuvo: {err}"),
                format!("El servidor en el puerto {port} se detuvo."),
            )
        })
}

async fn ingest(
    state: std::sync::Arc<AppState>,
    reset: bool,
    force: bool,
    status: bool,
    file: Option<String>,
) -> Result<i32, AppError> {
    if status {
        return show_status(&state).await;
    }
    let only = if let Some(raw) = file {
        Some(resolve_file(&raw, &state.documents.documents_directory)?)
    } else {
        None
    };
    if let Some(path) = &only {
        println!("[run] Indexando solo {}", path.display());
    } else {
        println!(
            "[run] Inicio de ingestión en {}",
            state.documents.documents_directory.display()
        );
    }
    let result = state.reindex(reset, force, only.as_deref()).await?;
    println!("{}", result.message);
    println!("Fragmentos escritos: {}", result.chunks_indexed);
    if only.is_none() {
        println!(
            "Archivos indexados: {} · omitidos: {}",
            result.files_indexed, result.files_skipped
        );
    }
    println!("Colección: {}", state.settings.chroma_collection_name);
    println!(
        "Persistencia: {}",
        state.settings.chroma_persist_directory.display()
    );
    Ok(0)
}

async fn show_status(state: &AppState) -> Result<i32, AppError> {
    if !state.vectors.lock().await.is_connected().await {
        println!("No se pudo conectar con la base vectorial.");
        return Ok(1);
    }
    let status = state.index_status().await?;
    println!("Carpeta:   {}", status.directory);
    println!("Colección: {}", state.settings.chroma_collection_name);
    println!("Chunks:    {}", status.indexed_chunks);
    println!("Modelo:    {}", state.settings.embedding_model);
    println!(
        "Pendiente: {}",
        if status.pending_changes { "sí" } else { "no" }
    );
    println!();
    if status.files.is_empty() {
        println!("No hay archivos .txt o .md para indexar.");
        return Ok(0);
    }
    println!(
        "{:<42} {:<10} {:>6}  Actualizado",
        "Archivo", "Estado", "Chunks"
    );
    for item in status.files {
        let label = match item.status.as_str() {
            "indexed" => "al día",
            "changed" => "cambió",
            "new" => "nuevo",
            "error" => "error",
            other => other,
        };
        let stamp = if item.indexed_at.is_empty() {
            "-"
        } else {
            item.indexed_at.as_str()
        };
        println!(
            "{:<42} {:<10} {:>6}  {stamp}",
            item.name, label, item.chunks
        );
    }
    Ok(0)
}

fn resolve_file(raw: &str, directory: &Path) -> Result<std::path::PathBuf, AppError> {
    let candidate = std::path::PathBuf::from(raw);
    if candidate.exists() {
        return Ok(candidate);
    }
    let nested = directory.join(raw);
    if nested.exists() {
        return Ok(nested);
    }
    Err(AppError::document(
        format!("No encontré el archivo: {raw}"),
        format!("No encontré el archivo: {raw}"),
    ))
}
