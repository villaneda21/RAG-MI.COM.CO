//! Conexión perezosa a PostgreSQL. Si al arrancar no hay base, el siguiente
//! request vuelve a intentar y el mensaje explica qué falta.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{Mutex, RwLock};

use super::store::Helpdesk;
use super::worker::spawn_inactivity_worker;

#[derive(Clone)]
pub struct HelpdeskHandle {
    slot: Arc<RwLock<Option<Helpdesk>>>,
    error: Arc<RwLock<String>>,
    last_failure: Arc<Mutex<Option<Instant>>>,
    worker_started: Arc<AtomicBool>,
    database_url: String,
    media_dir: PathBuf,
}

impl HelpdeskHandle {
    pub fn new(database_url: String, media_dir: PathBuf, initial: Option<Helpdesk>) -> Self {
        Self {
            slot: Arc::new(RwLock::new(initial)),
            error: Arc::new(RwLock::new(String::new())),
            last_failure: Arc::new(Mutex::new(None)),
            worker_started: Arc::new(AtomicBool::new(false)),
            database_url,
            media_dir,
        }
    }

    pub async fn connected(&self) -> bool {
        self.slot.read().await.is_some()
    }

    pub async fn get(&self) -> Result<Helpdesk, String> {
        if let Some(ready) = self.slot.read().await.clone() {
            return Ok(ready);
        }
        let cooling_down = self
            .last_failure
            .lock()
            .await
            .is_some_and(|instant| instant.elapsed() < Duration::from_secs(5));
        if cooling_down {
            return Err(self.error.read().await.clone());
        }
        self.establish().await
    }

    pub async fn establish(&self) -> Result<Helpdesk, String> {
        if let Some(ready) = self.slot.read().await.clone() {
            self.start_worker(&ready);
            return Ok(ready);
        }
        match Helpdesk::connect(&self.database_url, self.media_dir.clone()).await {
            Ok(helpdesk) => {
                self.start_worker(&helpdesk);
                *self.slot.write().await = Some(helpdesk.clone());
                self.error.write().await.clear();
                *self.last_failure.lock().await = None;
                tracing::info!(
                    "Centro de ayuda conectado. El temporizador revisa inactividad cada minuto."
                );
                Ok(helpdesk)
            }
            Err(err) => {
                tracing::error!("{err}");
                *self.error.write().await = err.clone();
                *self.last_failure.lock().await = Some(Instant::now());
                Err(err)
            }
        }
    }

    fn start_worker(&self, helpdesk: &Helpdesk) {
        if self
            .worker_started
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            spawn_inactivity_worker(helpdesk.pool.clone(), self.media_dir.clone());
        }
    }
}
