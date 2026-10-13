-- Esquema relacional del asistente de soporte de MI.COM.CO.
-- Reemplaza ChromaDB (fragmentos) y los JSON/JSONL de casos y conversaciones.
-- PostgreSQL 14 o superior. El rol de migración debe poder crear pg_trgm.

CREATE EXTENSION IF NOT EXISTS pg_trgm;

CREATE TABLE knowledge_documents (
    id UUID PRIMARY KEY,
    source TEXT NOT NULL UNIQUE,
    sha256 TEXT NOT NULL CHECK (char_length(sha256) = 64),
    characters INTEGER NOT NULL CHECK (characters >= 0),
    indexed_at TIMESTAMPTZ NOT NULL
);

CREATE TABLE knowledge_chunks (
    id UUID PRIMARY KEY,
    document_id UUID NOT NULL REFERENCES knowledge_documents (id) ON DELETE CASCADE,
    chunk_index INTEGER NOT NULL CHECK (chunk_index > 0),
    doc_chunk_id TEXT,
    doc_chunk_part INTEGER,
    title TEXT NOT NULL DEFAULT '',
    title_folded TEXT NOT NULL DEFAULT '',
    category TEXT NOT NULL DEFAULT '',
    category_folded TEXT NOT NULL DEFAULT '',
    entity TEXT NOT NULL DEFAULT '',
    keywords TEXT[] NOT NULL DEFAULT '{}',
    keywords_folded TEXT NOT NULL DEFAULT '',
    summary TEXT NOT NULL DEFAULT '',
    summary_folded TEXT NOT NULL DEFAULT '',
    content TEXT NOT NULL CHECK (char_length(content) > 0),
    content_folded TEXT NOT NULL,
    source TEXT NOT NULL,
    search_document TEXT GENERATED ALWAYS AS (
        title_folded || ' ' || title_folded || ' ' || keywords_folded || ' ' ||
        category_folded || ' ' || summary_folded || ' ' || content_folded
    ) STORED,
    search_vector tsvector GENERATED ALWAYS AS (
        setweight(to_tsvector('spanish', title_folded), 'A') ||
        setweight(to_tsvector('spanish', keywords_folded), 'A') ||
        setweight(to_tsvector('spanish', category_folded), 'B') ||
        setweight(to_tsvector('spanish', summary_folded), 'B') ||
        setweight(to_tsvector('spanish', content_folded), 'C')
    ) STORED,
    UNIQUE (document_id, chunk_index)
);

CREATE INDEX knowledge_chunks_search_idx ON knowledge_chunks USING GIN (search_vector);
CREATE INDEX knowledge_chunks_trgm_idx ON knowledge_chunks USING GIN (search_document gin_trgm_ops);
CREATE INDEX knowledge_chunks_doc_chunk_idx ON knowledge_chunks (doc_chunk_id);
CREATE INDEX knowledge_chunks_category_idx ON knowledge_chunks (category_folded);

CREATE TABLE conversations (
    id UUID PRIMARY KEY,
    session_id TEXT NOT NULL UNIQUE CHECK (char_length(session_id) BETWEEN 6 AND 96),
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('open', 'closed')),
    case_id TEXT NOT NULL DEFAULT '',
    correo TEXT NOT NULL DEFAULT '',
    categoria TEXT NOT NULL DEFAULT '',
    summary TEXT NOT NULL DEFAULT ''
);

CREATE INDEX conversations_updated_idx ON conversations (updated_at DESC);

CREATE TABLE conversation_messages (
    id UUID PRIMARY KEY,
    conversation_id UUID NOT NULL REFERENCES conversations (id) ON DELETE CASCADE,
    position INTEGER NOT NULL CHECK (position >= 0),
    role TEXT NOT NULL CHECK (role IN ('user', 'assistant')),
    content TEXT NOT NULL CHECK (char_length(content) BETWEEN 1 AND 16000),
    created_at TIMESTAMPTZ NOT NULL,
    UNIQUE (conversation_id, position)
);

CREATE TABLE support_cases (
    id UUID PRIMARY KEY,
    case_id TEXT NOT NULL UNIQUE,
    correo TEXT NOT NULL CHECK (correo ~ '^[^@[:space:]]+@[^@[:space:]]+\.[^@[:space:]]+$'),
    categoria TEXT NOT NULL CHECK (categoria IN (
        'Correo electrónico',
        'Dominio',
        'Hosting',
        'Facturación o Compras'
    )),
    detalle TEXT NOT NULL CHECK (char_length(detalle) BETWEEN 3 AND 4000),
    registrado_en TIMESTAMPTZ NOT NULL,
    estado TEXT NOT NULL CHECK (estado = 'Cerrado/Registrado'),
    session_id TEXT NOT NULL DEFAULT ''
);

CREATE INDEX support_cases_registrado_idx ON support_cases (registrado_en DESC);
