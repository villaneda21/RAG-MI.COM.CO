//! Lectura de la carpeta `data/documents/`. Porta `app/services/document_service.py`.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::config::{chunk_options, Settings};
use crate::error::AppError;
use crate::models::TextChunk;
use crate::text::{clean_text, split_into_chunks, ChunkOptions};

const KNOWLEDGE_SUFFIXES: &[&str] = &[".txt", ".md"];
const SKIP_FILENAMES: &[&str] = &["readme.md"];

pub fn list_knowledge_files(directory: &Path, fallback: Option<&Path>) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if directory.is_dir() {
        if let Ok(entries) = std::fs::read_dir(directory) {
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_file() {
                    continue;
                }
                let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
                    continue;
                };
                if name.starts_with('.') || name.starts_with('_') {
                    continue;
                }
                if SKIP_FILENAMES.contains(&name.to_lowercase().as_str()) {
                    continue;
                }
                let suffix = path
                    .extension()
                    .and_then(|value| value.to_str())
                    .map(|value| format!(".{value}").to_lowercase())
                    .unwrap_or_default();
                if !KNOWLEDGE_SUFFIXES.contains(&suffix.as_str()) {
                    continue;
                }
                files.push(path);
            }
        }
    }
    files.sort_by(|left, right| left.file_name().cmp(&right.file_name()));
    if !files.is_empty() {
        return files;
    }
    if let Some(fallback) = fallback {
        if fallback.is_file() {
            return vec![fallback.to_path_buf()];
        }
    }
    Vec::new()
}

#[derive(Debug, Clone)]
pub struct DocumentService {
    pub document_path: PathBuf,
    pub documents_directory: PathBuf,
    pub chunk_options: ChunkOptions,
}

impl DocumentService {
    pub fn new(settings: &Settings) -> Self {
        Self {
            document_path: settings.default_document_path.clone(),
            documents_directory: settings.documents_directory.clone(),
            chunk_options: chunk_options(settings),
        }
    }

    pub fn for_file(path: PathBuf, directory: PathBuf, options: ChunkOptions) -> Self {
        Self {
            document_path: path,
            documents_directory: directory,
            chunk_options: options,
        }
    }

    pub fn list_knowledge_files(&self) -> Vec<PathBuf> {
        list_knowledge_files(&self.documents_directory, Some(&self.document_path))
    }

    pub fn load_raw_text(&self) -> Result<String, AppError> {
        read_document(&self.document_path)
    }

    pub fn fingerprint(&self, raw: &str) -> String {
        fingerprint_text(raw)
    }

    pub fn chunk_text(&self, raw: &str, source: &str) -> Result<Vec<TextChunk>, AppError> {
        let cleaned = clean_text(raw);
        tracing::info!(characters = cleaned.chars().count(), "texto limpio");
        let chunks = split_into_chunks(&cleaned, source, self.chunk_options)?;
        if chunks.is_empty() {
            return Err(AppError::document(
                "No se generaron fragmentos a partir del documento.",
                "No fue posible dividir el documento en fragmentos útiles.",
            ));
        }
        tracing::info!(count = chunks.len(), %source, "chunks creados");
        Ok(chunks)
    }

    pub fn process(&self) -> Result<(Vec<TextChunk>, usize, String), AppError> {
        let raw = self.load_raw_text()?;
        let fingerprint = self.fingerprint(&raw);
        let chunks = self.chunk_text(&raw, &file_name(&self.document_path))?;
        Ok((chunks, raw.chars().count(), fingerprint))
    }
}

pub fn fingerprint_text(raw: &str) -> String {
    format!("{:x}", Sha256::digest(raw.as_bytes()))
}

pub fn read_document(path: &Path) -> Result<String, AppError> {
    tracing::info!(path = %path.display(), "carga del documento");
    if !path.exists() {
        return Err(AppError::document(
            format!("No existe el archivo de conocimiento: {}", path.display()),
            "No se encontró el archivo de conocimiento. Coloca un TXT o MD en data/documents/ y vuelve a indexar.",
        ));
    }
    let bytes = std::fs::read(path).map_err(|err| {
        AppError::document(
            format!("No se pudo leer el documento: {err}"),
            "No se pudo leer el archivo de conocimiento. Verifica permisos y formato.",
        )
    })?;
    let raw = match String::from_utf8(bytes.clone()) {
        Ok(text) => text,
        Err(_) => {
            tracing::warn!(path = %path.display(), "UTF-8 falló; se reintenta con latin-1");
            bytes.iter().map(|byte| *byte as char).collect()
        }
    };
    if raw.trim().is_empty() {
        return Err(AppError::document(
            format!("El archivo está vacío: {}", path.display()),
            "El archivo de conocimiento está vacío. Agrégale contenido antes de indexar.",
        ));
    }
    tracing::info!(characters = raw.chars().count(), "documento leído");
    Ok(raw)
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("documento.txt")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::list_knowledge_files;
    use crate::documents::DocumentService;
    use crate::text::ChunkOptions;
    use std::path::PathBuf;

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("micomco-docs-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn list_skips_templates_and_readme() {
        let dir = temp_dir();
        std::fs::write(dir.join("documento.txt"), "política de la empresa").unwrap();
        std::fs::write(dir.join("_plantilla_guia.txt"), "no indexar").unwrap();
        std::fs::write(dir.join("README.md"), "# hola").unwrap();
        std::fs::write(dir.join("guia_nueva.md"), "Cómo renovar un dominio.").unwrap();
        let names: Vec<String> = list_knowledge_files(&dir, None)
            .iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert!(names.contains(&"documento.txt".to_string()));
        assert!(names.contains(&"guia_nueva.md".to_string()));
        assert!(!names.iter().any(|name| name.starts_with('_')));
        assert!(!names
            .iter()
            .any(|name| name.eq_ignore_ascii_case("readme.md")));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn empty_and_missing_files_fail() {
        let dir = temp_dir();
        let empty = dir.join("vacio.txt");
        std::fs::write(&empty, "   \n").unwrap();
        let service = DocumentService::for_file(empty, dir.clone(), ChunkOptions::default());
        assert!(service.load_raw_text().is_err());
        let missing = DocumentService::for_file(
            dir.join("no-existe.txt"),
            dir.clone(),
            ChunkOptions::default(),
        );
        assert!(missing.load_raw_text().is_err());
        let _ = std::fs::remove_dir_all(dir);
    }
}
