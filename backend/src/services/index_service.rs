use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::domain::models::{IndexFileStatus, IndexStatusResponse, ReindexResponse};
use crate::domain::text::{
    fold_search, sha256_hex, split_into_chunks, ChunkSettings, Patterns, TextChunk,
};
use crate::domain::timeutil::format_bogota;
use crate::error::AppError;
use crate::repositories::knowledge::{KnowledgeRepository, NewChunk};
use crate::state::AppState;

pub async fn index_if_empty(state: &AppState) -> Result<(), AppError> {
    let count = KnowledgeRepository::new(&state.pool).count_chunks().await?;
    if count > 0 {
        tracing::info!(chunks = count, "la base de conocimiento ya está indexada");
        return Ok(());
    }
    match reindex(state, false, false).await {
        Ok(result) => {
            tracing::info!(
                chunks = result.chunks_indexed,
                message = %result.message,
                "indexación inicial"
            );
            Ok(())
        }
        Err(error @ AppError::Document(_)) => {
            tracing::warn!(error = %error, "sin documentos para la indexación inicial");
            Ok(())
        }
        Err(error) => Err(error),
    }
}

pub async fn reindex(
    state: &AppState,
    reset: bool,
    force: bool,
) -> Result<ReindexResponse, AppError> {
    let files = list_knowledge_files(&state.config.documents_dir, &state.config.fallback_document);
    if files.is_empty() {
        return Err(AppError::Document(
            "No hay archivos .txt o .md en data/documents/. Copia tu guía ahí y vuelve a indexar."
                .into(),
        ));
    }
    let repository = KnowledgeRepository::new(&state.pool);
    if reset {
        repository.clear_all().await?;
    }
    let patterns = Patterns::compile().map_err(|err| {
        AppError::Internal(format!(
            "No se pudieron compilar los patrones de texto: {err}"
        ))
    })?;
    let settings = chunk_settings(&state.config);
    let mut written = 0_i64;
    let mut characters = 0_i64;
    let mut indexed_names = Vec::new();
    let mut skipped_names = Vec::new();

    for path in files {
        let source = file_name(&path);
        let raw = read_document(&path)?;
        let fingerprint = sha256_hex(&raw);
        let file_characters = i32::try_from(raw.chars().count()).map_err(|_| {
            AppError::Document("El documento es demasiado grande para indexarlo.".into())
        })?;
        characters += i64::from(file_characters);
        if !reset && !force {
            if let Some(meta) = repository.source_meta(&source).await? {
                if meta.sha256 == fingerprint && meta.chunks > 0 {
                    skipped_names.push(source);
                    continue;
                }
            }
        }
        let chunks = split_into_chunks(&raw, &source, &patterns, &settings)
            .map_err(|err| AppError::Document(err.to_string()))?;
        let stored: Vec<NewChunk> = chunks.iter().map(store_chunk).collect();
        repository
            .replace_source(&source, &fingerprint, file_characters, &stored)
            .await?;
        written += i64::try_from(chunks.len()).unwrap_or(i64::MAX);
        indexed_names.push(source);
    }

    let listed = if indexed_names.is_empty() {
        skipped_names.clone()
    } else {
        indexed_names.clone()
    };
    let message = if indexed_names.is_empty() {
        "Nada que indexar: los archivos ya estaban al día.".to_string()
    } else if reset {
        format!(
            "Base recreada. Se indexaron {} archivo(s) ({} fragmentos).",
            indexed_names.len(),
            written
        )
    } else {
        let mut text = format!(
            "Se indexaron {} archivo(s) con cambios ({} fragmentos).",
            indexed_names.len(),
            written
        );
        if !skipped_names.is_empty() {
            text.push_str(&format!(" Sin cambios: {}.", skipped_names.join(", ")));
        }
        text
    };

    Ok(ReindexResponse {
        status: "ok".into(),
        message,
        chunks_indexed: written,
        characters,
        source: listed.join(", "),
        sources: listed,
        files_indexed: i64::try_from(indexed_names.len()).unwrap_or(i64::MAX),
        files_skipped: i64::try_from(skipped_names.len()).unwrap_or(i64::MAX),
        reset,
    })
}

pub async fn index_status(state: &AppState) -> Result<IndexStatusResponse, AppError> {
    let repository = KnowledgeRepository::new(&state.pool);
    let files = list_knowledge_files(&state.config.documents_dir, &state.config.fallback_document);
    let mut items = Vec::new();
    let mut pending = false;
    for path in &files {
        let name = file_name(path);
        let raw = match read_document(path) {
            Ok(raw) => raw,
            Err(_) => {
                pending = true;
                items.push(IndexFileStatus {
                    name,
                    status: "error".into(),
                    chunks: 0,
                    characters: 0,
                    indexed_at: String::new(),
                });
                continue;
            }
        };
        let fingerprint = sha256_hex(&raw);
        let characters = i64::try_from(raw.chars().count()).unwrap_or(i64::MAX);
        let meta = repository.source_meta(&name).await?;
        let status = match &meta {
            None => {
                pending = true;
                "new"
            }
            Some(meta) if meta.sha256 != fingerprint => {
                pending = true;
                "changed"
            }
            Some(_) => "indexed",
        };
        items.push(IndexFileStatus {
            name,
            status: status.into(),
            chunks: meta.as_ref().map(|item| item.chunks).unwrap_or(0),
            characters,
            indexed_at: meta
                .as_ref()
                .map(|item| format_bogota(item.indexed_at))
                .unwrap_or_default(),
        });
    }
    let directory = files
        .first()
        .and_then(|path| path.parent())
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| state.config.documents_dir.display().to_string());
    Ok(IndexStatusResponse {
        directory,
        indexed_chunks: repository.count_chunks().await?,
        pending_changes: pending,
        files: items,
    })
}

pub fn list_knowledge_files(directory: &Path, fallback: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if directory.is_dir() {
        if let Ok(entries) = std::fs::read_dir(directory) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() && is_knowledge_file(&path) {
                    files.push(path);
                }
            }
        }
        files.sort();
    }
    if files.is_empty() && fallback.is_file() {
        files.push(fallback.to_path_buf());
    }
    files
}

pub fn read_document(path: &Path) -> Result<String, AppError> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Err(AppError::Document(
                "No se encontró el archivo de conocimiento. Coloca un TXT o MD en data/documents/ y vuelve a indexar.".into(),
            ));
        }
        Err(err) => {
            tracing::error!(error = %err, path = %path.display(), "no se pudo leer el documento");
            return Err(AppError::Document(
                "No se pudo leer el archivo de conocimiento. Verifica permisos y formato.".into(),
            ));
        }
    };
    if bytes.is_empty() {
        return Err(AppError::Document(
            "El archivo de conocimiento está vacío. Agrégale contenido antes de indexar.".into(),
        ));
    }
    match String::from_utf8(bytes) {
        Ok(text) => Ok(text),
        Err(err) => Ok(err
            .into_bytes()
            .into_iter()
            .map(|byte| byte as char)
            .collect()),
    }
}

fn store_chunk(chunk: &TextChunk) -> NewChunk {
    NewChunk {
        chunk_index: chunk.chunk_index,
        doc_chunk_id: chunk.doc_chunk_id.clone(),
        doc_chunk_part: chunk.doc_chunk_part,
        title: chunk.title.clone(),
        title_folded: fold_search(&chunk.title),
        category: chunk.category.clone(),
        category_folded: fold_search(&chunk.category),
        entity: chunk.entity.clone(),
        keywords: chunk.keywords.clone(),
        keywords_folded: fold_search(&format!("{} {}", chunk.keywords.join(" "), chunk.entity)),
        summary: chunk.summary.clone(),
        summary_folded: fold_search(&chunk.summary),
        content: chunk.content.clone(),
        content_folded: fold_search(&chunk.content),
        source: chunk.source.clone(),
    }
}

fn chunk_settings(config: &Config) -> ChunkSettings {
    ChunkSettings {
        chunk_size_min: config.chunk_size_min,
        chunk_size_max: config.chunk_size_max,
        chunk_overlap: config.chunk_overlap,
        structured_max: 8_000,
    }
}

fn is_knowledge_file(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
        return false;
    };
    if name.starts_with('.') || name.starts_with('_') || name.eq_ignore_ascii_case("readme.md") {
        return false;
    }
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".txt") || lower.ends_with(".md")
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("documento.txt")
        .to_string()
}
