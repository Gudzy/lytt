//! Audio source abstraction for Lytt.
//!
//! Provides a trait-based interface for different audio sources (YouTube, local files, etc.).

mod local;
mod youtube;

pub use local::LocalSource;
pub use youtube::YoutubeSource;

use crate::error::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Type of media source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceType {
    YouTube,
    Local,
}

impl std::fmt::Display for SourceType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SourceType::YouTube => write!(f, "youtube"),
            SourceType::Local => write!(f, "local"),
        }
    }
}

/// Metadata about a media file (audio or video).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaMetadata {
    /// Unique identifier.
    pub id: String,
    /// Title.
    pub title: String,
    /// Description (if available).
    pub description: Option<String>,
    /// Duration in seconds (if known).
    pub duration_seconds: Option<u32>,
    /// Type of source.
    pub source_type: SourceType,
    /// URL or path to the media.
    pub source_url: String,
    /// Publication date (if available).
    pub published_at: Option<DateTime<Utc>>,
    /// Channel or author name (if available).
    pub channel: Option<String>,
    /// Thumbnail URL (if available).
    pub thumbnail_url: Option<String>,
}

impl MediaMetadata {
    /// Create a URL with timestamp for a specific point in the media.
    pub fn url_with_timestamp(&self, seconds: f64) -> String {
        match self.source_type {
            SourceType::YouTube => {
                format!("https://youtube.com/watch?v={}&t={}s", self.id, seconds as u32)
            }
            SourceType::Local => {
                format!("{}#t={}", self.source_url, seconds as u32)
            }
        }
    }

    /// Format seconds as MM:SS or HH:MM:SS.
    pub fn format_timestamp(seconds: f64) -> String {
        crate::util::format_seconds(seconds)
    }
}

/// Trait for audio source providers.
#[async_trait]
pub trait AudioSource: Send + Sync {
    /// Get the source type.
    fn source_type(&self) -> SourceType;

    /// Fetch metadata for media by ID.
    async fn fetch_media(&self, id: &str) -> Result<MediaMetadata>;

    /// List media from a source (channel, playlist, directory).
    async fn list_media(&self, source: &str, limit: Option<usize>) -> Result<Vec<MediaMetadata>>;

    /// Check if this source can handle the given input.
    fn can_handle(&self, input: &str) -> bool;

    /// Extract ID from input (URL, path, etc.).
    fn extract_id(&self, input: &str) -> Option<String>;
}

/// Download/extract audio from media.
///
/// This is a separate function rather than a trait method because
/// the download process is the same for all sources (via yt-dlp/ffmpeg).
pub async fn download_audio(media: &MediaMetadata, output_dir: &Path) -> Result<PathBuf> {
    crate::audio::download_audio(&media.source_url, &media.id, output_dir).await
}

/// Detect the appropriate audio source for the given input.
pub fn detect_source(input: &str) -> Option<Box<dyn AudioSource>> {
    let youtube = YoutubeSource::new();
    if youtube.can_handle(input) {
        return Some(Box::new(youtube));
    }

    let local = LocalSource::new();
    if local.can_handle(input) {
        return Some(Box::new(local));
    }

    None
}

/// Parse input and return the appropriate source and ID.
pub fn parse_input(input: &str) -> Option<(Box<dyn AudioSource>, String)> {
    let source = detect_source(input)?;
    let id = source.extract_id(input)?;
    Some((source, id))
}
