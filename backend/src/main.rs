#[tokio::main]
async fn main() {
    if let Err(err) = micomco_backend::run().await {
        eprintln!("No se pudo iniciar el backend: {err}");
        std::process::exit(1);
    }
}
