# ONNX Runtime (fastembed / ort-sys) referencia __isoc23_strtol y
# std::__cxx11::basic_string::_M_replace_cold. Esos símbolos existen en
# glibc >= 2.38 y en la libstdc++ de Debian 13. Bookworm (glibc 2.36) no
# enlaza el binario.
FROM rust:1-trixie AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY src ./src
COPY migrations ./migrations
COPY templates ./templates
COPY static ./static
RUN cargo build --release --bin rag-micomco

FROM debian:trixie-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates libstdc++6 \
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
