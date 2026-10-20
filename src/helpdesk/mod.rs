//! Centro de ayuda: CRM, WhatsApp de clientes y temporizador del bot.

mod http;
mod link;
mod logic;
mod service;
mod store;
mod worker;

pub use http::routes;
pub use link::HelpdeskHandle;
pub use store::Helpdesk;
pub use worker::{run_inactivity_pass, spawn_inactivity_worker};
