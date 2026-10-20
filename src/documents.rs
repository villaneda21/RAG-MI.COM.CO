//! Lectura de la carpeta `data/documents/`.

use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::config::Settings;
use crate::models::TextChunk;
use crate::text::{clean_text, split_into_chunks, DocumentProcessingError};

const KNOWLEDGE_SUFFIXES: &[&str] = &["txt", "md"];
const SKIP_FILENAMES: &[&str] = &["readme.md"];

pub fn list_knowledge_files(directory: &Path, fallback: Option<&Path>) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if directory.is_dir() {
        if let Ok(entries) = fs::read_dir(directory) {
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_file() {
                    continue;
                }
                let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                    continue;
                };
                let lowered = name.to_lowercase();
                if name.starts_with('.') || name.starts_with('_') {
                    continue;
                }
                if SKIP_FILENAMES.contains(&lowered.as_str()) {
                    continue;
                }
                let suffix = path
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .unwrap_or("")
                    .to_lowercase();
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
    files
}

#[derive(Debug, Clone)]
pub struct DocumentService {
    pub document_path: PathBuf,
    pub documents_directory: PathBuf,
    pub chunk_size_min: usize,
    pub chunk_size_max: usize,
    pub chunk_overlap: usize,
}

impl DocumentService {
    pub fn from_settings(settings: &Settings) -> Self {
        Self {
            document_path: settings.default_document_path.clone(),
            documents_directory: settings.documents_directory.clone(),
            chunk_size_min: settings.chunk_size_min,
            chunk_size_max: settings.chunk_size_max,
            chunk_overlap: settings.chunk_overlap,
        }
    }

    pub fn for_file(&self, path: &Path) -> Self {
        Self {
            document_path: path.to_path_buf(),
            documents_directory: self.documents_directory.clone(),
            chunk_size_min: self.chunk_size_min,
            chunk_size_max: self.chunk_size_max,
            chunk_overlap: self.chunk_overlap,
        }
    }

    pub fn list_knowledge_files(&self) -> Vec<PathBuf> {
        list_knowledge_files(&self.documents_directory, Some(&self.document_path))
    }

    pub fn load_raw_text(&self) -> Result<String, DocumentProcessingError> {
        let path = &self.document_path;
        tracing::info!("Carga del documento: {}", path.display());
        if !path.exists() {
            return Err(DocumentProcessingError::new(
                format!("No existe el archivo de conocimiento: {}", path.display()),
                "No se encontró el archivo de conocimiento. Coloca un TXT o MD en data/documents/ y vuelve a indexar.",
            ));
        }
        let bytes = fs::read(path).map_err(|err| {
            tracing::error!("Error de E/S al leer el documento: {err}");
            DocumentProcessingError::new(
                format!("No se pudo leer el documento: {err}"),
                "No se pudo leer el archivo de conocimiento. Verifica permisos y formato.",
            )
        })?;
        let raw = match String::from_utf8(bytes.clone()) {
            Ok(text) => text,
            Err(_) => {
                tracing::warn!(
                    "UTF-8 falló para {}; se reintenta con latin-1.",
                    path.display()
                );
                bytes.iter().map(|byte| *byte as char).collect()
            }
        };
        if raw.trim().is_empty() {
            return Err(DocumentProcessingError::new(
                format!("El archivo está vacío: {}", path.display()),
                "El archivo de conocimiento está vacío. Agrégale contenido antes de indexar.",
            ));
        }
        tracing::info!("Documento leído: {} caracteres.", raw.chars().count());
        Ok(raw)
    }

    pub fn fingerprint(&self, raw: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(raw.as_bytes());
        hex::encode(hasher.finalize())
    }

    pub fn chunk_text(
        &self,
        raw: &str,
        source: Option<&str>,
    ) -> Result<Vec<TextChunk>, DocumentProcessingError> {
        let cleaned = clean_text(raw);
        tracing::info!("Texto limpio: {} caracteres.", cleaned.chars().count());
        let origin = source.unwrap_or_else(|| {
            self.document_path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("documento.txt")
        });
        let chunks = split_into_chunks(
            cleaned.as_str(),
            origin,
            self.chunk_size_min,
            self.chunk_size_max,
            self.chunk_overlap,
        )?;
        if chunks.is_empty() {
            return Err(DocumentProcessingError::new(
                "No se generaron fragmentos a partir del documento.",
                "No fue posible dividir el documento en fragmentos útiles.",
            ));
        }
        tracing::info!("Chunks creados: {} (origen={origin}).", chunks.len());
        Ok(chunks)
    }

    pub fn process(&self) -> Result<(Vec<TextChunk>, usize, String), DocumentProcessingError> {
        let raw = self.load_raw_text()?;
        let characters = raw.chars().count();
        let name = self
            .document_path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("documento.txt")
            .to_string();
        let fingerprint = self.fingerprint(&raw);
        let chunks = self.chunk_text(&raw, Some(&name))?;
        Ok((chunks, characters, fingerprint))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_sample(path: &Path) {
        fs::write(
            path,
            "La política de la empresa indica que el soporte técnico responde en horario continuo.\n\n\
El código de cliente tiene el formato CLI-XXXXXX y se consulta en Mi cuenta.\n\n\
Para acceder a cPanel se debe entrar a Mis servicios y pulsar Acceder a cPanel.\n",
        )
        .unwrap();
    }

    #[test]
    fn read_txt_and_reject_empty_or_missing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("documento.txt");
        write_sample(&path);
        let service = DocumentService {
            document_path: path,
            documents_directory: dir.path().to_path_buf(),
            chunk_size_min: 800,
            chunk_size_max: 1200,
            chunk_overlap: 200,
        };
        let raw = service.load_raw_text().unwrap();
        assert!(raw.contains("política de la empresa"));

        let empty = dir.path().join("vacio.txt");
        fs::write(&empty, "   \n").unwrap();
        let empty_service = DocumentService {
            document_path: empty,
            ..service.clone()
        };
        assert!(empty_service.load_raw_text().is_err());

        let missing = DocumentService {
            document_path: dir.path().join("no-existe.txt"),
            ..service
        };
        assert!(missing.load_raw_text().is_err());
    }

    #[test]
    fn list_knowledge_files_skips_templates() {
        let dir = tempfile::tempdir().unwrap();
        write_sample(&dir.path().join("documento.txt"));
        fs::write(dir.path().join("_plantilla_guia.txt"), "no indexar").unwrap();
        fs::write(dir.path().join("README.md"), "# hola").unwrap();
        fs::write(
            dir.path().join("guia_nueva.md"),
            "Cómo renovar un dominio paso a paso.",
        )
        .unwrap();
        let names: Vec<String> = list_knowledge_files(dir.path(), None)
            .iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert!(names.contains(&"documento.txt".to_string()));
        assert!(names.contains(&"guia_nueva.md".to_string()));
        assert!(!names.iter().any(|name| name == "_plantilla_guia.txt"));
        assert!(!names
            .iter()
            .any(|name| name.eq_ignore_ascii_case("README.md")));
    }
}
