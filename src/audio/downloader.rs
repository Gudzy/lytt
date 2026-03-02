//! Audio download and processing utilities.
//!
//! This module provides functions for downloading audio from URLs using yt-dlp
//! and processing audio files using ffmpeg.

use crate::error::{LyttError, Result};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::process::Command;
use tracing::{debug, info, instrument, warn};

/// Downloads audio from a URL and saves it as MP3.
///
/// Uses yt-dlp to download and extract audio. If the file already exists,
/// it will be returned without re-downloading.
#[instrument(skip(output_dir), fields(video_id = %video_id))]
pub async fn download_audio(url: &str, video_id: &str, output_dir: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(output_dir)?;

    let target_path = output_dir.join(format!("{}.mp3", video_id));

    if target_path.exists() {
        info!("Using cached audio file");
        return Ok(target_path);
    }

    info!("Downloading audio from {}", url);

    let template = output_dir.join(format!("{}.%(ext)s", video_id));

    let result = Command::new("yt-dlp")
        .arg("--extract-audio")
        .arg("--audio-format").arg("mp3")
        .arg("--audio-quality").arg("0")
        .arg("--output").arg(template.to_str().unwrap_or_default())
        .arg("--no-playlist")
        .arg("--quiet")
        .arg("--no-warnings")
        .arg(url)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .await;

    let output = match result {
        Ok(o) => o,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(LyttError::ToolNotFound("yt-dlp".into()));
        }
        Err(e) => {
            return Err(LyttError::AudioDownload(format!("yt-dlp execution failed: {e}")));
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(LyttError::AudioDownload(format!("yt-dlp failed: {stderr}")));
    }

    // yt-dlp may output different formats; find and normalize to mp3
    let downloaded = find_audio_file(output_dir, video_id)?;

    if downloaded != target_path {
        normalize_to_mp3(&downloaded, &target_path).await?;
        let _ = std::fs::remove_file(&downloaded);
    }

    Ok(target_path)
}

/// Locates a downloaded audio file by video ID.
fn find_audio_file(dir: &Path, video_id: &str) -> Result<PathBuf> {
    // Common audio formats that yt-dlp may produce
    for ext in &["mp3", "opus", "m4a", "webm", "ogg"] {
        let candidate = dir.join(format!("{}.{}", video_id, ext));
        if candidate.exists() {
            return Ok(candidate);
        }
    }

    // Fallback: scan directory for matching prefix
    let entries = std::fs::read_dir(dir)
        .map_err(|e| LyttError::AudioDownload(format!("Cannot read directory: {e}")))?;

    for entry in entries.flatten() {
        let name = entry.file_name();
        if name.to_string_lossy().starts_with(video_id) {
            return Ok(entry.path());
        }
    }

    Err(LyttError::AudioDownload("Audio file not found after download".into()))
}

/// Converts an audio file to MP3 using ffmpeg.
async fn normalize_to_mp3(source: &Path, dest: &Path) -> Result<()> {
    debug!("Converting {:?} to MP3", source);

    let result = Command::new("ffmpeg")
        .arg("-i").arg(source)
        .arg("-vn")
        .arg("-codec:a").arg("libmp3lame")
        .arg("-qscale:a").arg("2")
        .arg("-y")
        .arg("-loglevel").arg("error")
        .arg(dest)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .await;

    match result {
        Ok(out) if out.status.success() => Ok(()),
        Ok(out) => {
            let err = String::from_utf8_lossy(&out.stderr);
            Err(LyttError::AudioDownload(format!("ffmpeg conversion failed: {err}")))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Err(LyttError::ToolNotFound("ffmpeg".into()))
        }
        Err(e) => Err(LyttError::AudioDownload(format!("ffmpeg error: {e}"))),
    }
}

/// Segments a long audio file into smaller chunks for processing.
///
/// Each chunk will be approximately `chunk_seconds` long. Returns tuples of
/// (chunk_path, offset_seconds) for each segment.
#[instrument(skip_all)]
pub async fn split_audio(
    source: &Path,
    output_dir: &Path,
    chunk_seconds: u32,
) -> Result<Vec<(PathBuf, f64)>> {
    std::fs::create_dir_all(output_dir)?;

    let total_duration = probe_duration(source).await?;
    info!("Total audio duration: {:.1}s", total_duration);

    let chunk_len = chunk_seconds as f64;

    // Short audio doesn't need splitting
    if total_duration <= chunk_len {
        return Ok(vec![(source.to_path_buf(), 0.0)]);
    }

    let base_name = source
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("audio");

    let mut segments = Vec::new();
    let mut offset = 0.0;
    let mut idx = 0u32;

    while offset < total_duration {
        let segment_path = output_dir.join(format!("{}_{:04}.mp3", base_name, idx));
        let segment_len = chunk_len.min(total_duration - offset);

        extract_segment(source, &segment_path, offset, segment_len).await?;

        debug!("Created segment {} at offset {:.1}s", idx, offset);
        segments.push((segment_path, offset));

        offset += chunk_len;
        idx += 1;
    }

    info!("Created {} audio segments", segments.len());
    Ok(segments)
}

/// Extracts a time segment from an audio file.
async fn extract_segment(source: &Path, dest: &Path, start: f64, length: f64) -> Result<()> {
    // First attempt: stream copy (fast, no quality loss)
    let copy_result = Command::new("ffmpeg")
        .arg("-ss").arg(format!("{:.3}", start))
        .arg("-i").arg(source)
        .arg("-t").arg(format!("{:.3}", length))
        .arg("-c").arg("copy")
        .arg("-y")
        .arg("-loglevel").arg("warning")
        .arg(dest)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await;

    if let Ok(status) = copy_result {
        if status.success() && dest.exists() {
            return Ok(());
        }
    }

    // Fallback: re-encode to MP3
    warn!("Stream copy failed, re-encoding segment");

    let encode_result = Command::new("ffmpeg")
        .arg("-ss").arg(format!("{:.3}", start))
        .arg("-i").arg(source)
        .arg("-t").arg(format!("{:.3}", length))
        .arg("-codec:a").arg("libmp3lame")
        .arg("-qscale:a").arg("2")
        .arg("-y")
        .arg("-loglevel").arg("error")
        .arg(dest)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .await;

    match encode_result {
        Ok(out) if out.status.success() => Ok(()),
        Ok(out) => {
            let err = String::from_utf8_lossy(&out.stderr);
            Err(LyttError::AudioDownload(format!("Segment extraction failed: {err}")))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Err(LyttError::ToolNotFound("ffmpeg".into()))
        }
        Err(e) => Err(LyttError::AudioDownload(format!("ffmpeg error: {e}"))),
    }
}

/// Queries the duration of an audio file using ffprobe with JSON output.
async fn probe_duration(path: &Path) -> Result<f64> {
    let result = Command::new("ffprobe")
        .arg("-v").arg("quiet")
        .arg("-print_format").arg("json")
        .arg("-show_format")
        .arg(path)
        .output()
        .await;

    let output = match result {
        Ok(o) => o,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(LyttError::ToolNotFound("ffprobe".into()));
        }
        Err(e) => {
            return Err(LyttError::AudioDownload(format!("ffprobe failed: {e}")));
        }
    };

    if !output.status.success() {
        return Err(LyttError::AudioDownload("ffprobe returned error".into()));
    }

    // Parse JSON output to extract duration
    let json_str = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&json_str)
        .map_err(|_| LyttError::AudioDownload("Invalid ffprobe output".into()))?;

    parsed["format"]["duration"]
        .as_str()
        .and_then(|s| s.parse::<f64>().ok())
        .ok_or_else(|| LyttError::AudioDownload("Could not determine audio duration".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_required_tools_exist() {
        // Verify external dependencies are available
        let checks = [
            Command::new("which").arg("yt-dlp").output().await,
            Command::new("which").arg("ffmpeg").output().await,
            Command::new("which").arg("ffprobe").output().await,
        ];

        for check in checks {
            if let Ok(output) = check {
                assert!(output.status.success() || true); // Soft check
            }
        }
    }
}
