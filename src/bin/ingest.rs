//! Indexa `data/documents/` en la base vectorial.
//!
//! Sustituye a `python scripts/ingest_document.py`.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;

use rag_micomco::build_rag;
use rag_micomco::config::Settings;
use rag_micomco::vector::VectorStoreError;

#[derive(Debug, Parser)]
#[command(about = "Indexa la carpeta data/documents/ en la base vectorial.")]
struct Args {
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
    #[arg(long, visible_alias = "document")]
    file: Option<String>,
}

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
    let args = Args::parse();
    let settings = Settings::load();
    let rag = match build_rag(settings.clone()) {
        Ok(rag) => rag,
        Err(err) => {
            eprintln!("{}", err.user_message);
            return ExitCode::from(1);
        }
    };

    if args.status {
        return match show_status(&rag).await {
            Ok(code) => code,
            Err(err) => {
                eprintln!("{}", err.user_message);
                ExitCode::from(1)
            }
        };
    }

    let only = if let Some(raw) = args.file {
        match resolve_file(&raw, &rag.documents.documents_directory) {
            Ok(path) => Some(path),
            Err(message) => {
                eprintln!("{message}");
                return ExitCode::from(1);
            }
        }
    } else {
        None
    };

    if let Err(err) = rag.vector.connect().await {
        eprintln!("{}", err.user_message);
        return ExitCode::from(1);
    }
    match rag.reindex(args.reset, args.force, only).await {
        Ok(result) => {
            println!("{}", result.message);
            println!("Fragmentos escritos: {}", result.chunks_indexed);
            println!(
                "Archivos indexados: {} · omitidos: {}",
                result.files_indexed, result.files_skipped
            );
            println!("Colección: {}", rag.vector.collection_name());
            println!(
                "Persistencia: {}",
                settings.chroma_persist_directory.display()
            );
            ExitCode::SUCCESS
        }
        Err(err) => {
            let message = match err {
                rag_micomco::rag::RagError::Document(error) => error.user_message,
                rag_micomco::rag::RagError::Vector(error) => error.user_message,
                other => other.to_string(),
            };
            eprintln!("{message}");
            ExitCode::from(1)
        }
    }
}

async fn show_status(rag: &rag_micomco::rag::RagService) -> Result<ExitCode, VectorStoreError> {
    rag.vector.connect().await?;
    let status = rag.index_status().await;
    println!("Carpeta:  {}", status.directory);
    println!("Colección: {}", rag.vector.collection_name());
    println!("Chunks:   {}", status.indexed_chunks);
    println!("Modelo:   {}", rag.settings.embedding_model);
    println!(
        "Pendiente: {}",
        if status.pending_changes { "sí" } else { "no" }
    );
    println!();
    if status.files.is_empty() {
        println!("No hay archivos .txt o .md para indexar.");
        return Ok(ExitCode::SUCCESS);
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
            "-".to_string()
        } else {
            item.indexed_at
        };
        println!(
            "{:<42} {:<10} {:>6}  {stamp}",
            item.name, label, item.chunks
        );
    }
    Ok(ExitCode::SUCCESS)
}

fn resolve_file(raw: &str, directory: &std::path::Path) -> Result<PathBuf, String> {
    let candidate = PathBuf::from(raw);
    if candidate.exists() {
        return Ok(candidate);
    }
    let nested = directory.join(raw);
    if nested.exists() {
        return Ok(nested);
    }
    Err(format!("No encontré el archivo: {raw}"))
}
