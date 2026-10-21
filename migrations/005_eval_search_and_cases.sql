-- Búsqueda léxica calibrada y casos confirmados por el cliente.

CREATE TABLE IF NOT EXISTS knowledge_chunks (
    chunk_id TEXT PRIMARY KEY,
    title TEXT NOT NULL DEFAULT '',
    keywords TEXT NOT NULL DEFAULT '',
    body TEXT NOT NULL DEFAULT '',
    search_vector tsvector GENERATED ALWAYS AS (
        setweight(to_tsvector('simple', coalesce(title, '')), 'A')
        || setweight(to_tsvector('simple', coalesce(keywords, '')), 'B')
        || setweight(to_tsvector('simple', coalesce(body, '')), 'C')
    ) STORED
);

CREATE INDEX IF NOT EXISTS knowledge_chunks_search_idx
    ON knowledge_chunks USING GIN (search_vector);

CREATE TABLE IF NOT EXISTS support_cases (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    case_code TEXT NOT NULL UNIQUE,
    ticket_id UUID NOT NULL UNIQUE REFERENCES tickets (id),
    customer_email TEXT NOT NULL,
    category TEXT NOT NULL,
    detail TEXT NOT NULL,
    guide_id TEXT,
    status TEXT NOT NULL DEFAULT 'Cerrado/Registrado',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS support_cases_email_idx ON support_cases (customer_email);
