# Binario de release del centro de ayuda. La primera ejecución descarga el
# modelo de embeddings si el índice local todavía no existe.
FROM rust:1-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY src ./src
COPY migrations ./migrations
COPY templates ./templates
COPY static ./static
RUN cargo build --release --bin rag-micomco

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=build /src/target/release/rag-micomco /usr/local/bin/rag-micomco
COPY templates ./templates
COPY static ./static
COPY data/documents ./data/documents
COPY migrations ./migrations
ENV RAG_PROJECT_ROOT=/app
ENV HOST=0.0.0.0
ENV PORT=8000
EXPOSE 8000
CMD ["rag-micomco"]
