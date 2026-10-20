//! Asistente RAG de MI.COM.CO.
//!
//! Sustituye el backend FastAPI (`run.py` / `app/`) por un servidor Axum:
//! recupera fragmentos de `data/documents/`, responde con Claude y guarda
//! casos y conversaciones en disco.

pub mod cases;
pub mod claude;
pub mod config;
pub mod conversations;
pub mod documents;
pub mod embeddings;
pub mod error;
pub mod models;
pub mod rag;
pub mod routes;
pub mod state;
pub mod text;
pub mod tls;
pub mod vector;
