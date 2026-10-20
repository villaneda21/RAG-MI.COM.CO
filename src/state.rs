//! Estado compartido del servidor HTTP.

use std::sync::Arc;

use tera::Tera;

use crate::helpdesk::{Helpdesk, HelpdeskHandle};
use crate::rag::RagService;

#[derive(Clone)]
pub struct AppState {
    pub rag: RagService,
    pub templates: Arc<Tera>,
    pub helpdesk: HelpdeskHandle,
}

impl AppState {
    pub fn try_new(rag: RagService) -> Result<Self, String> {
        Self::try_with_helpdesk(rag, None)
    }

    pub fn try_with_helpdesk(rag: RagService, helpdesk: Option<Helpdesk>) -> Result<Self, String> {
        let path = rag.settings.templates_dir.join("index.html");
        let mut tera = Tera::default();
        tera.add_template_file(&path, Some("index.html"))
            .map_err(|err| format!("No se pudo cargar {}: {err}", path.display()))?;
        let handle = HelpdeskHandle::new(
            rag.settings.database_url.clone(),
            rag.settings.media_dir.clone(),
            helpdesk,
        );
        Ok(Self {
            rag,
            templates: Arc::new(tera),
            helpdesk: handle,
        })
    }
}
