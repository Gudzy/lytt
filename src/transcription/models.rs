//! Data models for transcription.

use serde::{Deserialize, Serialize};

// ============================================================================
// Word-level and Fusion Types
// ============================================================================

/// A single word with precise timing from Whisper word-level timestamps.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WhisperWord {
    /// The word text.
    pub word: String,
    /// Start time in seconds.
    pub start: f64,
    /// End time in seconds.
    pub end: f64,
}

/// Word-level transcription result from Whisper.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WordLevelTranscript {
    /// Media ID.
    pub media_id: String,
    /// All words with timestamps.
    pub words: Vec<WhisperWord>,
    /// Full text (concatenated words).
    pub full_text: String,
    /// Total duration in seconds.
    pub duration_seconds: f64,
}

impl WordLevelTranscript {
    /// Create a new word-level transcript.
    pub fn new(media_id: String, words: Vec<WhisperWord>) -> Self {
        let full_text = words
            .iter()
            .map(|w| w.word.as_str())
            .collect::<Vec<_>>()
            .join(" ");

        let duration_seconds = words.last().map(|w| w.end).unwrap_or(0.0);

        Self {
            media_id,
            words,
            full_text,
            duration_seconds,
        }
    }

    /// Serialize words to JSON for fusion prompt.
    pub fn words_to_json(&self) -> String {
        serde_json::to_string_pretty(&self.words).unwrap_or_else(|_| "[]".to_string())
    }
}

/// Plain text transcription result (for GPT-4o-audio-preview).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlainTranscript {
    /// Media ID.
    pub media_id: String,
    /// Full transcript text.
    pub text: String,
    /// Language detected (if available).
    pub language: Option<String>,
}

/// Fused segment from LLM fusion.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FusedSegment {
    /// Segment text (from accurate transcript, possibly corrected).
    pub text: String,
    /// Start time in seconds (from Whisper).
    pub start_seconds: f64,
    /// End time in seconds (from Whisper).
    pub end_seconds: f64,
}

impl From<FusedSegment> for TranscriptSegment {
    fn from(fused: FusedSegment) -> Self {
        TranscriptSegment::new(fused.start_seconds, fused.end_seconds, fused.text)
    }
}

// ============================================================================
// Core Transcript Types
// ============================================================================

/// A complete transcript with segments.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transcript {
    /// Video ID this transcript belongs to.
    pub video_id: String,
    /// Individual transcript segments with timestamps.
    pub segments: Vec<TranscriptSegment>,
    /// Full transcript text (concatenated segments).
    pub full_text: String,
    /// Total duration in seconds.
    pub duration_seconds: f64,
}

impl Transcript {
    /// Create a new transcript from segments.
    pub fn new(video_id: String, segments: Vec<TranscriptSegment>) -> Self {
        let full_text = segments
            .iter()
            .map(|s| s.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");

        let duration_seconds = segments
            .iter()
            .map(|s| s.end_seconds)
            .fold(0.0_f64, f64::max);

        Self {
            video_id,
            segments,
            full_text,
            duration_seconds,
        }
    }

    /// Get the text content between two timestamps.
    pub fn text_between(&self, start: f64, end: f64) -> String {
        self.segments
            .iter()
            .filter(|s| s.start_seconds >= start && s.end_seconds <= end)
            .map(|s| s.text.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Format the transcript with timestamps for display.
    pub fn format_with_timestamps(&self) -> String {
        self.segments
            .iter()
            .map(|s| format!("[{} - {}] {}",
                format_timestamp(s.start_seconds),
                format_timestamp(s.end_seconds),
                s.text
            ))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// A single segment of a transcript with timestamp information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptSegment {
    /// Start time in seconds.
    pub start_seconds: f64,
    /// End time in seconds.
    pub end_seconds: f64,
    /// Transcribed text content.
    pub text: String,
}

impl TranscriptSegment {
    /// Create a new transcript segment.
    pub fn new(start_seconds: f64, end_seconds: f64, text: String) -> Self {
        Self {
            start_seconds,
            end_seconds,
            text,
        }
    }

    /// Duration of this segment in seconds.
    pub fn duration(&self) -> f64 {
        self.end_seconds - self.start_seconds
    }
}

/// Format seconds as MM:SS or HH:MM:SS.
pub fn format_timestamp(seconds: f64) -> String {
    crate::util::format_seconds(seconds)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_transcript_creation() {
        let segments = vec![
            TranscriptSegment::new(0.0, 5.0, "Hello world".to_string()),
            TranscriptSegment::new(5.0, 10.0, "This is a test".to_string()),
        ];

        let transcript = Transcript::new("test_video".to_string(), segments);

        assert_eq!(transcript.video_id, "test_video");
        assert_eq!(transcript.full_text, "Hello world This is a test");
        assert_eq!(transcript.duration_seconds, 10.0);
    }

    #[test]
    fn test_format_timestamp() {
        assert_eq!(format_timestamp(0.0), "00:00");
        assert_eq!(format_timestamp(65.0), "01:05");
        assert_eq!(format_timestamp(3665.0), "01:01:05");
    }

    #[test]
    fn test_text_between() {
        let segments = vec![
            TranscriptSegment::new(0.0, 5.0, "First".to_string()),
            TranscriptSegment::new(5.0, 10.0, "Second".to_string()),
            TranscriptSegment::new(10.0, 15.0, "Third".to_string()),
        ];

        let transcript = Transcript::new("test".to_string(), segments);
        assert_eq!(transcript.text_between(5.0, 10.0), "Second");
    }
}
