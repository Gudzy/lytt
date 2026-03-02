//! YouTube source implementation.

use super::{AudioSource, MediaMetadata, SourceType};
use crate::error::{Result, LyttError};
use async_trait::async_trait;
use regex::Regex;

/// YouTube audio source.
pub struct YoutubeSource {
    video_id_regex: Regex,
}

impl YoutubeSource {
    pub fn new() -> Self {
        // Matches various YouTube URL formats and bare video IDs
        let video_id_regex = Regex::new(
            r"(?x)
            (?:
                # Full YouTube URLs
                (?:https?://)?
                (?:www\.)?
                (?:youtube\.com/watch\?v=|youtu\.be/|youtube\.com/embed/|youtube\.com/v/)
                ([a-zA-Z0-9_-]{11})
            )
            |
            # Bare video ID (11 characters)
            ^([a-zA-Z0-9_-]{11})$
        ",
        )
        .expect("Invalid regex");

        Self { video_id_regex }
    }

    /// Extract video ID from a YouTube URL or bare ID.
    fn extract_video_id(&self, input: &str) -> Option<String> {
        let caps = self.video_id_regex.captures(input.trim())?;

        // Try group 1 (URL format) then group 2 (bare ID)
        caps.get(1)
            .or_else(|| caps.get(2))
            .map(|m| m.as_str().to_string())
    }

    /// Fetch metadata using yt-dlp.
    async fn fetch_metadata_ytdlp(&self, video_id: &str) -> Result<MediaMetadata> {
        let url = format!("https://www.youtube.com/watch?v={}", video_id);

        let output = tokio::process::Command::new("yt-dlp")
            .args([
                "--dump-json",
                "--no-download",
                "--no-warnings",
                "--ignore-errors",
                &url,
            ])
            .output()
            .await
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    LyttError::ToolNotFound("yt-dlp".to_string())
                } else {
                    LyttError::VideoSource(format!("Failed to run yt-dlp: {}", e))
                }
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(LyttError::VideoNotFound(format!(
                "Video {} not found or unavailable: {}",
                video_id, stderr
            )));
        }

        let json_str = String::from_utf8_lossy(&output.stdout);
        let json: serde_json::Value = serde_json::from_str(&json_str).map_err(|e| {
            LyttError::VideoSource(format!("Failed to parse yt-dlp output: {}", e))
        })?;

        let title = json["title"]
            .as_str()
            .unwrap_or("Unknown Title")
            .to_string();

        let description = json["description"].as_str().map(|s| s.to_string());

        let duration = json["duration"].as_f64().map(|d| d as u32);

        let channel = json["channel"]
            .as_str()
            .or_else(|| json["uploader"].as_str())
            .map(|s| s.to_string());

        let thumbnail = json["thumbnail"].as_str().map(|s| s.to_string());

        let published_at = json["upload_date"].as_str().and_then(|date_str| {
            // yt-dlp returns date as YYYYMMDD
            if date_str.len() == 8 {
                chrono::NaiveDate::parse_from_str(date_str, "%Y%m%d")
                    .ok()
                    .map(|d| {
                        d.and_hms_opt(0, 0, 0)
                            .unwrap()
                            .and_utc()
                    })
            } else {
                None
            }
        });

        Ok(MediaMetadata {
            id: video_id.to_string(),
            title,
            description,
            duration_seconds: duration,
            source_type: SourceType::YouTube,
            source_url: url,
            published_at,
            channel,
            thumbnail_url: thumbnail,
        })
    }
}

impl Default for YoutubeSource {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl AudioSource for YoutubeSource {
    fn source_type(&self) -> SourceType {
        SourceType::YouTube
    }

    async fn fetch_media(&self, id: &str) -> Result<MediaMetadata> {
        let video_id = self.extract_video_id(id).ok_or_else(|| {
            LyttError::InvalidInput(format!("Invalid YouTube video ID or URL: {}", id))
        })?;

        self.fetch_metadata_ytdlp(&video_id).await
    }

    async fn list_media(&self, source: &str, limit: Option<usize>) -> Result<Vec<MediaMetadata>> {
        // For playlists/channels, use yt-dlp to get video list
        let limit_str = limit.map(|l| l.to_string()).unwrap_or_else(|| "50".to_string());

        let output = tokio::process::Command::new("yt-dlp")
            .args([
                "--dump-json",
                "--no-download",
                "--no-warnings",
                "--flat-playlist",
                "--playlist-end",
                &limit_str,
                source,
            ])
            .output()
            .await
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    LyttError::ToolNotFound("yt-dlp".to_string())
                } else {
                    LyttError::VideoSource(format!("Failed to run yt-dlp: {}", e))
                }
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(LyttError::VideoSource(format!(
                "Failed to list videos: {}",
                stderr
            )));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut media_list = Vec::new();

        for line in stdout.lines() {
            if line.trim().is_empty() {
                continue;
            }

            if let Ok(json) = serde_json::from_str::<serde_json::Value>(line) {
                let id = json["id"]
                    .as_str()
                    .or_else(|| json["url"].as_str())
                    .map(|s| {
                        // Extract video ID from URL if needed
                        self.extract_video_id(s).unwrap_or_else(|| s.to_string())
                    });

                if let Some(video_id) = id {
                    let title = json["title"]
                        .as_str()
                        .unwrap_or("Unknown Title")
                        .to_string();

                    media_list.push(MediaMetadata {
                        id: video_id.clone(),
                        title,
                        description: None,
                        duration_seconds: json["duration"].as_f64().map(|d| d as u32),
                        source_type: SourceType::YouTube,
                        source_url: format!("https://www.youtube.com/watch?v={}", video_id),
                        published_at: None,
                        channel: json["channel"].as_str().map(|s| s.to_string()),
                        thumbnail_url: None,
                    });
                }
            }
        }

        Ok(media_list)
    }

    fn can_handle(&self, input: &str) -> bool {
        self.extract_video_id(input).is_some()
            || input.contains("youtube.com/playlist")
            || input.contains("youtube.com/channel")
            || input.contains("youtube.com/@")
    }

    fn extract_id(&self, input: &str) -> Option<String> {
        self.extract_video_id(input)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_video_id() {
        let source = YoutubeSource::new();

        // Test various URL formats
        assert_eq!(
            source.extract_video_id("https://www.youtube.com/watch?v=dQw4w9WgXcQ"),
            Some("dQw4w9WgXcQ".to_string())
        );
        assert_eq!(
            source.extract_video_id("https://youtu.be/dQw4w9WgXcQ"),
            Some("dQw4w9WgXcQ".to_string())
        );
        assert_eq!(
            source.extract_video_id("https://youtube.com/embed/dQw4w9WgXcQ"),
            Some("dQw4w9WgXcQ".to_string())
        );
        assert_eq!(
            source.extract_video_id("dQw4w9WgXcQ"),
            Some("dQw4w9WgXcQ".to_string())
        );

        // Test invalid inputs
        assert_eq!(source.extract_video_id("not-a-video-id"), None);
        assert_eq!(source.extract_video_id(""), None);
    }

    #[test]
    fn test_can_handle() {
        let source = YoutubeSource::new();

        assert!(source.can_handle("dQw4w9WgXcQ"));
        assert!(source.can_handle("https://www.youtube.com/watch?v=dQw4w9WgXcQ"));
        assert!(source.can_handle("https://youtube.com/playlist?list=PLtest"));
        assert!(!source.can_handle("/path/to/video.mp4"));
    }
}
