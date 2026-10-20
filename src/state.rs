//! Estado compartido del proceso: servicios, plantillas y configuración.

use std::path::Path;
use std::sync::{Arc, Mutex};

use tera::Tera;

use crate::cases::CaseService;
use crate::claude::ClaudeService;
use crate::config::Settings;
use crate::conversations::ConversationService;
use crate::documents::DocumentService;
use crate::error::AppError;
use crate::vector::VectorService;

pub struct AppState {
    pub settings: Settings,
    pub documents: DocumentService,
    pub vectors: tokio::sync::Mutex<VectorService>,
    pub claude: ClaudeService,
    pub cases: Mutex<CaseService>,
    pub conversations: Mutex<ConversationService>,
    pub templates: Tera,
}

impl AppState {
    pub async fn init(settings: Settings) -> Result<Arc<Self>, AppError> {
        let documents = DocumentService::new(&settings);
        let claude = ClaudeService::new(&settings)?;
        let cases = CaseService::new(&settings);
        let conversations = ConversationService::new(&settings);
        let mut vectors = VectorService::new(&settings)?;
        if let Err(err) = vectors.connect().await {
            tracing::error!(
                error = %err,
                "la base vectorial no está disponible al iniciar. El health lo reportará."
            );
        } else {
            let chunks = vectors.count().await;
            tracing::info!(
                backend = vectors.backend_name(),
                chunks,
                "base vectorial lista"
            );
        }
        let _ = std::fs::create_dir_all(settings.project_root.join("static"));
        let _ = std::fs::create_dir_all(&settings.conversations_directory);
        let _ = std::fs::create_dir_all(&settings.chroma_persist_directory);
        let templates = load_templates(&settings.project_root)?;
        Ok(Arc::new(Self {
            settings,
            documents,
            vectors: tokio::sync::Mutex::new(vectors),
            claude,
            cases: Mutex::new(cases),
            conversations: Mutex::new(conversations),
            templates,
        }))
    }
}

fn load_templates(root: &Path) -> Result<Tera, AppError> {
    let mut tera = Tera::default();
    let path = root.join("templates").join("index.html");
    tera.add_template_file(&path, Some("index.html"))
        .map_err(|err| {
            AppError::internal(
                format!("No se pudo cargar {}: {err}", path.display()),
                "No se pudo cargar la interfaz.",
            )
        })?;
    Ok(tera)
}
