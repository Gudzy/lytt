//! Vector store abstraction for Lytt.
//!
//! Provides a trait-based interface for different vector database backends.

mod memory;
mod sqlite;

pub use memory::MemoryVectorStore;
pub use sqlite::SqliteVectorStore;

use crate::error::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A document stored in the vector database.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Document {
    /// Unique document ID.
    pub id: Uuid,
    /// Video ID this document belongs to.
    pub video_id: String,
    /// Video title.
    pub video_title: String,
    /// Section title (if chunked semantically).
    pub section_title: Option<String>,
    /// Text content of this chunk.
    pub content: String,
    /// Start time in the video (seconds).
    pub start_seconds: f64,
    /// End time in the video (seconds).
    pub end_seconds: f64,
    /// Embedding vector.
    pub embedding: Vec<f32>,
    /// Order of this chunk in the video.
    pub chunk_order: i32,
    /// When the source video was created/published.
    pub source_created_at: Option<DateTime<Utc>>,
    /// When this document was indexed.
    pub indexed_at: DateTime<Utc>,
}

impl Document {
    /// Create a new document.
    pub fn new(
        video_id: String,
        video_title: String,
        section_title: Option<String>,
        content: String,
        start_seconds: f64,
        end_seconds: f64,
        embedding: Vec<f32>,
        chunk_order: i32,
        source_created_at: Option<DateTime<Utc>>,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            video_id,
            video_title,
            section_title,
            content,
            start_seconds,
            end_seconds,
            embedding,
            chunk_order,
            source_created_at,
            indexed_at: Utc::now(),
        }
    }

    /// Format timestamp for display.
    pub fn format_timestamp(&self) -> String {
        crate::util::format_seconds(self.start_seconds)
    }
}

/// A search result with score.
#[derive(Debug, Clone)]
pub struct SearchResult {
    /// The matched document.
    pub document: Document,
    /// Similarity score (higher is better).
    pub score: f32,
}

/// Summary information about an indexed video.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexedVideo {
    /// Video ID.
    pub video_id: String,
    /// Video title.
    pub video_title: String,
    /// Number of indexed chunks.
    pub chunk_count: u32,
    /// Total duration in seconds.
    pub total_duration_seconds: f64,
    /// When the video was indexed.
    pub indexed_at: DateTime<Utc>,
}

/// Trait for vector store implementations.
#[async_trait]
pub trait VectorStore: Send + Sync {
    /// Store a document with its embedding.
    async fn upsert(&self, doc: &Document) -> Result<()>;

    /// Bulk upsert documents.
    async fn upsert_batch(&self, docs: &[Document]) -> Result<usize>;

    /// Search for similar documents.
    async fn search(&self, query_embedding: &[f32], limit: usize) -> Result<Vec<SearchResult>>;

    /// Search with a minimum similarity threshold.
    async fn search_with_threshold(
        &self,
        query_embedding: &[f32],
        limit: usize,
        min_score: f32,
    ) -> Result<Vec<SearchResult>>;

    /// Delete documents by video ID.
    async fn delete_by_video_id(&self, video_id: &str) -> Result<usize>;

    /// List all indexed videos.
    async fn list_videos(&self) -> Result<Vec<IndexedVideo>>;

    /// Get a specific video's information.
    async fn get_video(&self, video_id: &str) -> Result<Option<IndexedVideo>>;

    /// Check if a video is indexed.
    async fn is_video_indexed(&self, video_id: &str) -> Result<bool>;

    /// Get all documents for a video.
    async fn get_by_video_id(&self, video_id: &str) -> Result<Vec<Document>>;

    /// Get total document count.
    async fn document_count(&self) -> Result<usize>;
}

/// Compute cosine similarity between two vectors.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }

    let dot_product: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();

    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }

    dot_product / (norm_a * norm_b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cosine_similarity() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        assert!((cosine_similarity(&a, &b) - 1.0).abs() < 0.001);

        let c = vec![0.0, 1.0, 0.0];
        assert!((cosine_similarity(&a, &c)).abs() < 0.001);

        let d = vec![-1.0, 0.0, 0.0];
        assert!((cosine_similarity(&a, &d) + 1.0).abs() < 0.001);
    }

    #[test]
    fn test_document_timestamp_format() {
        let doc = Document::new(
            "test".to_string(),
            "Test Video".to_string(),
            None,
            "content".to_string(),
            125.0, // 2:05
            130.0,
            vec![],
            0,
            None,
        );

        assert_eq!(doc.format_timestamp(), "02:05");
    }
}
