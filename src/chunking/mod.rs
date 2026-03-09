//! Content chunking strategies for breaking transcripts into searchable segments.
//!
//! Provides different strategies for splitting transcripts into meaningful chunks.

mod semantic;
mod temporal;

pub use semantic::SemanticChunker;
pub use temporal::TemporalChunker;

use crate::config::Prompts;
use crate::error::Result;
use crate::transcription::Transcript;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// A chunk of content from a video transcript.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContentChunk {
    /// Section title (for semantic chunking) or auto-generated.
    pub title: Option<String>,
    /// Text content of this chunk.
    pub content: String,
    /// Start time in seconds.
    pub start_seconds: f64,
    /// End time in seconds.
    pub end_seconds: f64,
    /// Order of this chunk in the video.
    pub order: i32,
    /// Optional summary of the chunk content.
    pub summary: Option<String>,
}

impl ContentChunk {
    /// Create a new content chunk.
    pub fn new(
        title: Option<String>,
        content: String,
        start_seconds: f64,
        end_seconds: f64,
        order: i32,
    ) -> Self {
        Self {
            title,
            content,
            start_seconds,
            end_seconds,
            order,
            summary: None,
        }
    }

    /// Duration of this chunk in seconds.
    pub fn duration(&self) -> f64 {
        self.end_seconds - self.start_seconds
    }

    /// Format timestamp for display.
    pub fn format_timestamp(&self) -> String {
        crate::util::format_seconds(self.start_seconds)
    }
}

/// Chunking strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChunkingStrategy {
    /// LLM-based semantic chunking.
    Semantic,
    /// Time-based chunking.
    Temporal,
    /// Combination of both strategies.
    Hybrid,
}

impl std::str::FromStr for ChunkingStrategy {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "semantic" => Ok(ChunkingStrategy::Semantic),
            "temporal" => Ok(ChunkingStrategy::Temporal),
            "hybrid" => Ok(ChunkingStrategy::Hybrid),
            _ => Err(format!("Unknown chunking strategy: {}", s)),
        }
    }
}

/// Configuration for chunking.
#[derive(Debug, Clone)]
pub struct ChunkingConfig {
    /// Target chunk duration in seconds.
    pub target_duration: u32,
    /// Minimum chunk duration in seconds.
    pub min_duration: u32,
    /// Maximum chunk duration in seconds.
    pub max_duration: u32,
}

impl Default for ChunkingConfig {
    fn default() -> Self {
        Self {
            target_duration: 180,
            min_duration: 60,
            max_duration: 600,
        }
    }
}

/// Trait for content chunking implementations.
#[async_trait]
pub trait Chunker: Send + Sync {
    /// Split a transcript into content chunks.
    async fn chunk(&self, transcript: &Transcript, config: &ChunkingConfig) -> Result<Vec<ContentChunk>>;
}

/// Create a chunker based on the strategy.
pub fn create_chunker(strategy: ChunkingStrategy) -> Box<dyn Chunker> {
    match strategy {
        ChunkingStrategy::Semantic => Box::new(SemanticChunker::new()),
        ChunkingStrategy::Temporal => Box::new(TemporalChunker::new()),
        ChunkingStrategy::Hybrid => {
            // For hybrid, we use semantic as the primary chunker
            // with temporal as a fallback if semantic fails
            Box::new(SemanticChunker::new())
        }
    }
}

/// Create a chunker with custom prompts (including user-defined variables).
pub fn create_chunker_with_prompts(strategy: ChunkingStrategy, prompts: Prompts) -> Box<dyn Chunker> {
    match strategy {
        ChunkingStrategy::Semantic => Box::new(SemanticChunker::new().with_prompts(prompts)),
        ChunkingStrategy::Temporal => Box::new(TemporalChunker::new()),
        ChunkingStrategy::Hybrid => {
            Box::new(SemanticChunker::new().with_prompts(prompts))
        }
    }
}
