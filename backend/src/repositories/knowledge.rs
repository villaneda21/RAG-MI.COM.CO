use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::domain::models::RetrievedChunk;
use crate::domain::text::{extract_chunk_id, fold_search, single_like_token};
use crate::error::AppError;

const REINDEX_LOCK: i64 = 842_150_451;

#[derive(Debug, Clone)]
pub struct NewChunk {
    pub chunk_index: i32,
    pub doc_chunk_id: Option<String>,
    pub doc_chunk_part: Option<i32>,
    pub title: String,
    pub title_folded: String,
    pub category: String,
    pub category_folded: String,
    pub entity: String,
    pub keywords: Vec<String>,
    pub keywords_folded: String,
    pub summary: String,
    pub summary_folded: String,
    pub content: String,
    pub content_folded: String,
    pub source: String,
}

#[derive(Debug)]
pub struct SourceMeta {
    pub sha256: String,
    pub chunks: i64,
    pub indexed_at: DateTime<Utc>,
}

pub struct KnowledgeRepository<'a> {
    pool: &'a PgPool,
}

impl<'a> KnowledgeRepository<'a> {
    pub fn new(pool: &'a PgPool) -> Self {
        Self { pool }
    }

    pub async fn count_chunks(&self) -> Result<i64, AppError> {
        let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*)::bigint FROM knowledge_chunks")
            .fetch_one(self.pool)
            .await?;
        Ok(count)
    }

    pub async fn source_meta(&self, source: &str) -> Result<Option<SourceMeta>, AppError> {
        let row = sqlx::query_as::<_, SourceMetaRow>(
            r#"
            SELECT d.sha256,
                   d.indexed_at,
                   (SELECT COUNT(*)::bigint FROM knowledge_chunks c WHERE c.document_id = d.id) AS chunks
            FROM knowledge_documents d
            WHERE d.source = $1
            "#,
        )
        .bind(source)
        .fetch_optional(self.pool)
        .await?;
        Ok(row.map(|item| SourceMeta {
            sha256: item.sha256,
            chunks: item.chunks,
            indexed_at: item.indexed_at,
        }))
    }

    pub async fn clear_all(&self) -> Result<(), AppError> {
        let mut tx = self.pool.begin().await?;
        lock_reindex(&mut tx).await?;
        sqlx::query("DELETE FROM knowledge_documents")
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn replace_source(
        &self,
        source: &str,
        sha256: &str,
        characters: i32,
        chunks: &[NewChunk],
    ) -> Result<(), AppError> {
        let mut tx = self.pool.begin().await?;
        lock_reindex(&mut tx).await?;
        sqlx::query("DELETE FROM knowledge_documents WHERE source = $1")
            .bind(source)
            .execute(&mut *tx)
            .await?;
        let document_id = Uuid::new_v4();
        sqlx::query(
            r#"
            INSERT INTO knowledge_documents (id, source, sha256, characters, indexed_at)
            VALUES ($1, $2, $3, $4, $5)
            "#,
        )
        .bind(document_id)
        .bind(source)
        .bind(sha256)
        .bind(characters)
        .bind(Utc::now())
        .execute(&mut *tx)
        .await?;

        for chunk in chunks {
            sqlx::query(
                r#"
                INSERT INTO knowledge_chunks (
                    id, document_id, chunk_index, doc_chunk_id, doc_chunk_part,
                    title, title_folded, category, category_folded, entity,
                    keywords, keywords_folded, summary, summary_folded,
                    content, content_folded, source
                )
                VALUES (
                    $1, $2, $3, $4, $5,
                    $6, $7, $8, $9, $10,
                    $11, $12, $13, $14,
                    $15, $16, $17
                )
                "#,
            )
            .bind(Uuid::new_v4())
            .bind(document_id)
            .bind(chunk.chunk_index)
            .bind(&chunk.doc_chunk_id)
            .bind(chunk.doc_chunk_part)
            .bind(&chunk.title)
            .bind(&chunk.title_folded)
            .bind(&chunk.category)
            .bind(&chunk.category_folded)
            .bind(&chunk.entity)
            .bind(&chunk.keywords)
            .bind(&chunk.keywords_folded)
            .bind(&chunk.summary)
            .bind(&chunk.summary_folded)
            .bind(&chunk.content)
            .bind(&chunk.content_folded)
            .bind(&chunk.source)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn search(
        &self,
        query: &str,
        category: Option<&str>,
        limit: i64,
    ) -> Result<Vec<RetrievedChunk>, AppError> {
        let folded = take_chars(&fold_search(query), 400);
        if folded.is_empty() {
            return Ok(Vec::new());
        }
        let chunk_id = extract_chunk_id(&folded);
        let like_token = single_like_token(&folded);
        let category_folded = category
            .map(fold_search)
            .filter(|value| !value.is_empty())
            .unwrap_or_default();
        let rows = sqlx::query_as::<_, ChunkRow>(
            r#"
            SELECT
                c.chunk_index,
                c.doc_chunk_id,
                c.doc_chunk_part,
                c.title,
                c.category,
                c.entity,
                c.keywords,
                c.summary,
                c.content,
                c.source,
                (
                    ts_rank_cd(c.search_vector, websearch_to_tsquery('spanish', $1)) * 2
                    + word_similarity($1, c.search_document)
                    + CASE
                        WHEN $2 <> '' AND upper(c.doc_chunk_id) = upper($2) THEN 5
                        ELSE 0
                      END
                    + CASE
                        WHEN $3 <> '' AND c.title_folded ILIKE '%' || $3 || '%' THEN 0.4
                        ELSE 0
                      END
                )::float8 AS score
            FROM knowledge_chunks c
            WHERE (
                c.search_vector @@ websearch_to_tsquery('spanish', $1)
                OR word_similarity($1, c.search_document) > 0.18
                OR ($2 <> '' AND upper(c.doc_chunk_id) = upper($2))
                OR ($3 <> '' AND c.title_folded ILIKE '%' || $3 || '%')
            )
            AND ($4 = '' OR c.category_folded = $4)
            ORDER BY score DESC, c.chunk_index ASC
            LIMIT $5
            "#,
        )
        .bind(&folded)
        .bind(&chunk_id)
        .bind(&like_token)
        .bind(&category_folded)
        .bind(limit.clamp(1, 50))
        .fetch_all(self.pool)
        .await?;
        Ok(rows.into_iter().map(ChunkRow::into_chunk).collect())
    }

    pub async fn list(
        &self,
        category: Option<&str>,
        limit: i64,
    ) -> Result<Vec<RetrievedChunk>, AppError> {
        let category_folded = category
            .map(fold_search)
            .filter(|value| !value.is_empty())
            .unwrap_or_default();
        let rows = sqlx::query_as::<_, ChunkRow>(
            r#"
            SELECT
                chunk_index, doc_chunk_id, doc_chunk_part, title, category, entity,
                keywords, summary, content, source, 0::float8 AS score
            FROM knowledge_chunks
            WHERE ($1 = '' OR category_folded = $1)
            ORDER BY doc_chunk_id NULLS LAST, doc_chunk_part NULLS FIRST, chunk_index
            LIMIT $2
            "#,
        )
        .bind(&category_folded)
        .bind(limit.clamp(1, 100))
        .fetch_all(self.pool)
        .await?;
        Ok(rows.into_iter().map(ChunkRow::into_chunk).collect())
    }

    pub async fn by_doc_chunk_id(
        &self,
        doc_chunk_id: &str,
    ) -> Result<Vec<RetrievedChunk>, AppError> {
        let rows = sqlx::query_as::<_, ChunkRow>(
            r#"
            SELECT
                chunk_index, doc_chunk_id, doc_chunk_part, title, category, entity,
                keywords, summary, content, source, 0::float8 AS score
            FROM knowledge_chunks
            WHERE upper(doc_chunk_id) = upper($1)
            ORDER BY doc_chunk_part NULLS FIRST, chunk_index
            "#,
        )
        .bind(doc_chunk_id.trim())
        .fetch_all(self.pool)
        .await?;
        Ok(rows.into_iter().map(ChunkRow::into_chunk).collect())
    }
}

async fn lock_reindex(tx: &mut sqlx::Transaction<'_, sqlx::Postgres>) -> Result<(), AppError> {
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(REINDEX_LOCK)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

fn take_chars(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

#[derive(sqlx::FromRow)]
struct SourceMetaRow {
    sha256: String,
    indexed_at: DateTime<Utc>,
    chunks: i64,
}

#[derive(sqlx::FromRow)]
struct ChunkRow {
    chunk_index: i32,
    doc_chunk_id: Option<String>,
    doc_chunk_part: Option<i32>,
    title: String,
    category: String,
    entity: String,
    keywords: Vec<String>,
    summary: String,
    content: String,
    source: String,
    score: f64,
}

impl ChunkRow {
    fn into_chunk(self) -> RetrievedChunk {
        RetrievedChunk {
            chunk_index: self.chunk_index,
            doc_chunk_id: self.doc_chunk_id,
            doc_chunk_part: self.doc_chunk_part,
            title: self.title,
            category: self.category,
            entity: self.entity,
            keywords: self.keywords,
            summary: self.summary,
            content: self.content,
            source: self.source,
            score: self.score,
        }
    }
}
