//! SQLite-based vector store implementation.
//!
//! Uses SQLite with cosine similarity computed in Rust for simplicity.
//! For production use cases with large datasets, consider using sqlite-vec extension
//! or a dedicated vector database.

use super::{cosine_similarity, Document, IndexedVideo, SearchResult, VectorStore};
use crate::error::{Result, LyttError};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use std::path::Path;
use std::sync::Mutex;
use tracing::{debug, info, instrument};

/// DDL executed once on connection open to create/migrate the schema.
const SCHEMA_SQL: &str = r#"
    CREATE TABLE IF NOT EXISTS documents (
        id TEXT PRIMARY KEY,
        video_id TEXT NOT NULL,
        video_title TEXT NOT NULL,
        section_title TEXT,
        content TEXT NOT NULL,
        start_seconds REAL NOT NULL,
        end_seconds REAL NOT NULL,
        embedding BLOB NOT NULL,
        chunk_order INTEGER NOT NULL,
        source_created_at TEXT,
        indexed_at TEXT NOT NULL
    );

    CREATE INDEX IF NOT EXISTS idx_documents_video_id ON documents(video_id);
    CREATE INDEX IF NOT EXISTS idx_documents_indexed_at ON documents(indexed_at);

    CREATE TABLE IF NOT EXISTS transcripts (
        video_id TEXT PRIMARY KEY,
        video_title TEXT NOT NULL,
        transcript_json TEXT NOT NULL,
        duration_seconds REAL NOT NULL,
        transcribed_at TEXT NOT NULL
    );

    CREATE TABLE IF NOT EXISTS summaries (
        media_id   TEXT PRIMARY KEY,
        summary    TEXT NOT NULL,
        created_at TEXT NOT NULL
    );
"#;

/// SQLite-based vector store.
pub struct SqliteVectorStore {
    conn: Mutex<Connection>,
}

impl SqliteVectorStore {
    /// Create a new SQLite vector store.
    #[instrument(skip_all)]
    pub fn new(path: &Path) -> Result<Self> {
        // Create parent directories if needed
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let conn = Connection::open(path)?;

        // Enable WAL mode for better concurrent performance
        conn.execute_batch("PRAGMA journal_mode=WAL;")?;

        conn.execute_batch(SCHEMA_SQL)?;

        info!("Initialized SQLite vector store at {:?}", path);

        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Create an in-memory SQLite vector store (useful for testing).
    pub fn in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;

        conn.execute_batch(SCHEMA_SQL)?;

        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Serialize embedding to bytes.
    fn embedding_to_bytes(embedding: &[f32]) -> Vec<u8> {
        embedding
            .iter()
            .flat_map(|f| f.to_le_bytes())
            .collect()
    }

    /// Deserialize embedding from bytes.
    fn bytes_to_embedding(bytes: &[u8]) -> Vec<f32> {
        bytes
            .chunks_exact(4)
            .map(|chunk| {
                let arr: [u8; 4] = chunk.try_into().unwrap_or_default();
                f32::from_le_bytes(arr)
            })
            .collect()
    }
}

#[async_trait]
impl VectorStore for SqliteVectorStore {
    #[instrument(skip(self, doc))]
    async fn upsert(&self, doc: &Document) -> Result<()> {
        let conn = self.conn.lock().map_err(|e| {
            LyttError::VectorStore(format!("Failed to acquire lock: {}", e))
        })?;

        let embedding_bytes = Self::embedding_to_bytes(&doc.embedding);

        conn.execute(
            r#"
            INSERT OR REPLACE INTO documents
            (id, video_id, video_title, section_title, content, start_seconds, end_seconds,
             embedding, chunk_order, source_created_at, indexed_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
            "#,
            params![
                doc.id.to_string(),
                doc.video_id,
                doc.video_title,
                doc.section_title,
                doc.content,
                doc.start_seconds,
                doc.end_seconds,
                embedding_bytes,
                doc.chunk_order,
                doc.source_created_at.map(|dt| dt.to_rfc3339()),
                doc.indexed_at.to_rfc3339(),
            ],
        )?;

        debug!("Upserted document {}", doc.id);
        Ok(())
    }

    #[instrument(skip(self, docs))]
    async fn upsert_batch(&self, docs: &[Document]) -> Result<usize> {
        let conn = self.conn.lock().map_err(|e| {
            LyttError::VectorStore(format!("Failed to acquire lock: {}", e))
        })?;

        let tx = conn.unchecked_transaction()?;

        for doc in docs {
            let embedding_bytes = Self::embedding_to_bytes(&doc.embedding);

            tx.execute(
                r#"
                INSERT OR REPLACE INTO documents
                (id, video_id, video_title, section_title, content, start_seconds, end_seconds,
                 embedding, chunk_order, source_created_at, indexed_at)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                "#,
                params![
                    doc.id.to_string(),
                    doc.video_id,
                    doc.video_title,
                    doc.section_title,
                    doc.content,
                    doc.start_seconds,
                    doc.end_seconds,
                    embedding_bytes,
                    doc.chunk_order,
                    doc.source_created_at.map(|dt| dt.to_rfc3339()),
                    doc.indexed_at.to_rfc3339(),
                ],
            )?;
        }

        tx.commit()?;
        info!("Batch upserted {} documents", docs.len());
        Ok(docs.len())
    }

    #[instrument(skip(self, query_embedding))]
    async fn search(&self, query_embedding: &[f32], limit: usize) -> Result<Vec<SearchResult>> {
        self.search_with_threshold(query_embedding, limit, 0.0).await
    }

    #[instrument(skip(self, query_embedding))]
    async fn search_with_threshold(
        &self,
        query_embedding: &[f32],
        limit: usize,
        min_score: f32,
    ) -> Result<Vec<SearchResult>> {
        let conn = self.conn.lock().map_err(|e| {
            LyttError::VectorStore(format!("Failed to acquire lock: {}", e))
        })?;

        let mut stmt = conn.prepare(
            r#"
            SELECT id, video_id, video_title, section_title, content,
                   start_seconds, end_seconds, embedding, chunk_order,
                   source_created_at, indexed_at
            FROM documents
            "#,
        )?;

        let docs = stmt.query_map([], |row| {
            let id_str: String = row.get(0)?;
            let embedding_bytes: Vec<u8> = row.get(7)?;
            let source_created_str: Option<String> = row.get(9)?;
            let indexed_at_str: String = row.get(10)?;

            Ok(Document {
                id: uuid::Uuid::parse_str(&id_str).map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
                })?,
                video_id: row.get(1)?,
                video_title: row.get(2)?,
                section_title: row.get(3)?,
                content: row.get(4)?,
                start_seconds: row.get(5)?,
                end_seconds: row.get(6)?,
                embedding: Self::bytes_to_embedding(&embedding_bytes),
                chunk_order: row.get(8)?,
                source_created_at: source_created_str.and_then(|s| DateTime::parse_from_rfc3339(&s).ok().map(|dt| dt.with_timezone(&Utc))),
                indexed_at: DateTime::parse_from_rfc3339(&indexed_at_str)
                    .map(|dt| dt.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now()),
            })
        })?;

        let mut results: Vec<SearchResult> = docs
            .filter_map(|doc_result| doc_result.ok())
            .map(|doc| {
                let score = cosine_similarity(query_embedding, &doc.embedding);
                SearchResult { document: doc, score }
            })
            .filter(|r| r.score >= min_score)
            .collect();

        // Sort by score descending
        results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        results.truncate(limit);

        debug!("Found {} matching documents", results.len());
        Ok(results)
    }

    #[instrument(skip(self))]
    async fn delete_by_video_id(&self, video_id: &str) -> Result<usize> {
        let conn = self.conn.lock().map_err(|e| {
            LyttError::VectorStore(format!("Failed to acquire lock: {}", e))
        })?;

        let deleted = conn.execute(
            "DELETE FROM documents WHERE video_id = ?1",
            params![video_id],
        )?;

        info!("Deleted {} documents for video {}", deleted, video_id);
        Ok(deleted)
    }

    #[instrument(skip(self))]
    async fn list_videos(&self) -> Result<Vec<IndexedVideo>> {
        let conn = self.conn.lock().map_err(|e| {
            LyttError::VectorStore(format!("Failed to acquire lock: {}", e))
        })?;

        let mut stmt = conn.prepare(
            r#"
            SELECT video_id, video_title, COUNT(*) as chunk_count,
                   MAX(end_seconds) as total_duration, MAX(indexed_at) as indexed_at
            FROM documents
            GROUP BY video_id
            ORDER BY indexed_at DESC
            "#,
        )?;

        let videos = stmt.query_map([], |row| {
            let indexed_at_str: String = row.get(4)?;
            Ok(IndexedVideo {
                video_id: row.get(0)?,
                video_title: row.get(1)?,
                chunk_count: row.get(2)?,
                total_duration_seconds: row.get(3)?,
                indexed_at: DateTime::parse_from_rfc3339(&indexed_at_str)
                    .map(|dt| dt.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now()),
            })
        })?;

        let result: Vec<IndexedVideo> = videos.filter_map(|v| v.ok()).collect();
        Ok(result)
    }

    #[instrument(skip(self))]
    async fn get_video(&self, video_id: &str) -> Result<Option<IndexedVideo>> {
        let conn = self.conn.lock().map_err(|e| {
            LyttError::VectorStore(format!("Failed to acquire lock: {}", e))
        })?;

        let mut stmt = conn.prepare(
            r#"
            SELECT video_id, video_title, COUNT(*) as chunk_count,
                   MAX(end_seconds) as total_duration, MAX(indexed_at) as indexed_at
            FROM documents
            WHERE video_id = ?1
            GROUP BY video_id
            "#,
        )?;

        let video = stmt.query_row(params![video_id], |row| {
            let indexed_at_str: String = row.get(4)?;
            Ok(IndexedVideo {
                video_id: row.get(0)?,
                video_title: row.get(1)?,
                chunk_count: row.get(2)?,
                total_duration_seconds: row.get(3)?,
                indexed_at: DateTime::parse_from_rfc3339(&indexed_at_str)
                    .map(|dt| dt.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now()),
            })
        });

        match video {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    async fn is_video_indexed(&self, video_id: &str) -> Result<bool> {
        let video = self.get_video(video_id).await?;
        Ok(video.is_some())
    }

    #[instrument(skip(self))]
    async fn get_by_video_id(&self, video_id: &str) -> Result<Vec<Document>> {
        let conn = self.conn.lock().map_err(|e| {
            LyttError::VectorStore(format!("Failed to acquire lock: {}", e))
        })?;

        let mut stmt = conn.prepare(
            r#"
            SELECT id, video_id, video_title, section_title, content,
                   start_seconds, end_seconds, embedding, chunk_order,
                   source_created_at, indexed_at
            FROM documents
            WHERE video_id = ?1
            ORDER BY chunk_order
            "#,
        )?;

        let docs = stmt.query_map(params![video_id], |row| {
            let id_str: String = row.get(0)?;
            let embedding_bytes: Vec<u8> = row.get(7)?;
            let source_created_str: Option<String> = row.get(9)?;
            let indexed_at_str: String = row.get(10)?;

            Ok(Document {
                id: uuid::Uuid::parse_str(&id_str).map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
                })?,
                video_id: row.get(1)?,
                video_title: row.get(2)?,
                section_title: row.get(3)?,
                content: row.get(4)?,
                start_seconds: row.get(5)?,
                end_seconds: row.get(6)?,
                embedding: Self::bytes_to_embedding(&embedding_bytes),
                chunk_order: row.get(8)?,
                source_created_at: source_created_str.and_then(|s| DateTime::parse_from_rfc3339(&s).ok().map(|dt| dt.with_timezone(&Utc))),
                indexed_at: DateTime::parse_from_rfc3339(&indexed_at_str)
                    .map(|dt| dt.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now()),
            })
        })?;

        let result: Vec<Document> = docs.filter_map(|d| d.ok()).collect();
        debug!("Found {} documents for video {}", result.len(), video_id);
        Ok(result)
    }

    async fn document_count(&self) -> Result<usize> {
        let conn = self.conn.lock().map_err(|e| {
            LyttError::VectorStore(format!("Failed to acquire lock: {}", e))
        })?;

        let count: i64 = conn.query_row("SELECT COUNT(*) FROM documents", [], |row| row.get(0))?;
        Ok(count as usize)
    }
}

// Transcript storage methods (not part of VectorStore trait)
impl SqliteVectorStore {
    /// Store a raw transcript for later rechunking.
    pub fn store_transcript(
        &self,
        video_id: &str,
        video_title: &str,
        transcript: &crate::transcription::Transcript,
    ) -> Result<()> {
        let conn = self.conn.lock().map_err(|e| {
            LyttError::VectorStore(format!("Failed to acquire lock: {}", e))
        })?;

        let transcript_json = serde_json::to_string(transcript)
            .map_err(|e| LyttError::VectorStore(format!("Failed to serialize transcript: {}", e)))?;

        conn.execute(
            r#"
            INSERT OR REPLACE INTO transcripts (video_id, video_title, transcript_json, duration_seconds, transcribed_at)
            VALUES (?1, ?2, ?3, ?4, ?5)
            "#,
            params![
                video_id,
                video_title,
                transcript_json,
                transcript.duration_seconds,
                Utc::now().to_rfc3339(),
            ],
        )?;

        info!("Stored transcript for video {}", video_id);
        Ok(())
    }

    /// Retrieve a stored transcript.
    pub fn get_transcript(&self, video_id: &str) -> Result<Option<(String, crate::transcription::Transcript)>> {
        let conn = self.conn.lock().map_err(|e| {
            LyttError::VectorStore(format!("Failed to acquire lock: {}", e))
        })?;

        let result = conn.query_row(
            "SELECT video_title, transcript_json FROM transcripts WHERE video_id = ?1",
            params![video_id],
            |row| {
                let title: String = row.get(0)?;
                let json: String = row.get(1)?;
                Ok((title, json))
            },
        );

        match result {
            Ok((title, json)) => {
                let transcript: crate::transcription::Transcript = serde_json::from_str(&json)
                    .map_err(|e| LyttError::VectorStore(format!("Failed to deserialize transcript: {}", e)))?;
                Ok(Some((title, transcript)))
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Check if a transcript is stored.
    pub fn has_transcript(&self, video_id: &str) -> Result<bool> {
        let conn = self.conn.lock().map_err(|e| {
            LyttError::VectorStore(format!("Failed to acquire lock: {}", e))
        })?;

        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM transcripts WHERE video_id = ?1",
            params![video_id],
            |row| row.get(0),
        )?;

        Ok(count > 0)
    }

    /// List all stored transcripts.
    pub fn list_transcripts(&self) -> Result<Vec<(String, String, f64)>> {
        let conn = self.conn.lock().map_err(|e| {
            LyttError::VectorStore(format!("Failed to acquire lock: {}", e))
        })?;

        let mut stmt = conn.prepare(
            "SELECT video_id, video_title, duration_seconds FROM transcripts ORDER BY transcribed_at DESC"
        )?;

        let rows = stmt.query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?;

        let result: Vec<(String, String, f64)> = rows.filter_map(|r| r.ok()).collect();
        Ok(result)
    }

    /// Retrieve a cached summary for a media ID.
    pub fn get_summary(&self, media_id: &str) -> Result<Option<String>> {
        let conn = self.conn.lock().map_err(|e| {
            LyttError::VectorStore(format!("Failed to acquire lock: {}", e))
        })?;

        let result = conn.query_row(
            "SELECT summary FROM summaries WHERE media_id = ?1",
            params![media_id],
            |row| row.get(0),
        );

        match result {
            Ok(s) => Ok(Some(s)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Cache a generated summary for a media ID.
    pub fn store_summary(&self, media_id: &str, summary: &str) -> Result<()> {
        let conn = self.conn.lock().map_err(|e| {
            LyttError::VectorStore(format!("Failed to acquire lock: {}", e))
        })?;

        conn.execute(
            "INSERT OR REPLACE INTO summaries (media_id, summary, created_at) VALUES (?1, ?2, ?3)",
            params![media_id, summary, Utc::now().to_rfc3339()],
        )?;

        info!("Cached summary for media {}", media_id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_sqlite_vector_store() {
        let store = SqliteVectorStore::in_memory().unwrap();

        let doc = Document::new(
            "video1".to_string(),
            "Test Video".to_string(),
            Some("Section 1".to_string()),
            "This is test content".to_string(),
            0.0,
            60.0,
            vec![1.0, 0.0, 0.0],
            0,
            None,
        );

        store.upsert(&doc).await.unwrap();

        let videos = store.list_videos().await.unwrap();
        assert_eq!(videos.len(), 1);
        assert_eq!(videos[0].video_id, "video1");

        let results = store.search(&[1.0, 0.0, 0.0], 10).await.unwrap();
        assert_eq!(results.len(), 1);
        assert!((results[0].score - 1.0).abs() < 0.001);

        let deleted = store.delete_by_video_id("video1").await.unwrap();
        assert_eq!(deleted, 1);

        let videos = store.list_videos().await.unwrap();
        assert!(videos.is_empty());
    }
}
