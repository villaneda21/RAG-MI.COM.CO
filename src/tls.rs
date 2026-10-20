//! Proveedor TLS de `ring` para el cliente reqwest.
//!
//! Activamos `rustls-tls-webpki-roots-no-provider` para no encender el
//! feature opcional `quinn` de reqwest (incompatible con Rust 1.83). El
//! proveedor criptográfico se instala aquí antes de construir el cliente.

pub fn install_ring_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}
