//! Pipeline orchestrator for Lytt.
//!
//! Coordinates the entire process from audio download to indexing.

use crate::audio::download_audio;
use crate::audio_source::{MediaMetadata, parse_input};
use crate::chunking::{ChunkingConfig, ChunkingStrategy, ContentChunk, create_chunker_with_prompts};
use crate::config::{Prompts, Settings, TranscriptionProcessingSettings, TranscriptionProvider};
use crate::embedding::{Embedder, OpenAIEmbedder};
use crate::error::{Result, LyttError};
use crate::transcription::{TranscriptionProcessor, Transcript, Transcriber};
use crate::vector_store::{Document, SqliteVectorStore, VectorStore};
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{info, instrument, warn};

/// The main orchestrator for the Lytt pipeline.
pub struct Orchestrator {
    settings: Settings,
    prompts: Prompts,
    transcriber: Arc<dyn Transcriber>,
    embedder: Arc<dyn Embedder>,
    vector_store: Arc<SqliteVectorStore>,
    temp_dir: PathBuf,
}

impl Orchestrator {
    /// Create a new orchestrator with default configuration.
    pub fn new(settings: Settings) -> Result<Self> {
        // Load prompts (with optional custom directory and variables)
        let prompts = Prompts::load(
            settings.prompts.custom_dir.as_deref(),
            Some(&settings.prompts.variables),
        )?;

        // Create transcription processor
        // Whisper mode: no secondary text model, just cleanup
        // Fusion mode: Whisper + GPT-4o + LLM fusion
        let processing_settings = match settings.transcription.provider {
            TranscriptionProvider::Whisper => {
                info!("Using Whisper with LLM cleanup ({})", settings.transcription.processing.cleanup_model);
                TranscriptionProcessingSettings {
                    timestamp_model: settings.transcription.model.clone(),
                    text_model: None, // No secondary model
                    cleanup_model: settings.transcription.processing.cleanup_model.clone(),
                    max_concurrent: settings.transcription.max_concurrent_chunks,
                }
            }
            TranscriptionProvider::Fusion => {
                let text_model = settings.transcription.processing.text_model.clone()
                    .unwrap_or_else(|| "gpt-4o-transcribe".to_string());
                info!(
                    "Using fusion transcription ({} + {} -> {})",
                    settings.transcription.processing.timestamp_model,
                    text_model,
                    settings.transcription.processing.cleanup_model
                );
                settings.transcription.processing.clone()
            }
        };

        let transcriber: Arc<dyn Transcriber> = Arc::new(TranscriptionProcessor::with_config(
            &processing_settings,
            &prompts.cleanup.system,
        )?);

        let embedder = Arc::new(OpenAIEmbedder::with_config(
            &settings.embedding.model,
            settings.embedding.dimensions as usize,
        ));

        let vector_store = Arc::new(SqliteVectorStore::new(&settings.sqlite_path())?);

        let temp_dir = settings.temp_dir();
        std::fs::create_dir_all(&temp_dir)?;

        Ok(Self {
            settings,
            prompts,
            transcriber,
            embedder,
            vector_store,
            temp_dir,
        })
    }

    /// Create an orchestrator with custom components.
    pub fn with_components(
        settings: Settings,
        prompts: Prompts,
        transcriber: Arc<dyn Transcriber>,
        embedder: Arc<dyn Embedder>,
        vector_store: Arc<SqliteVectorStore>,
    ) -> Result<Self> {
        let temp_dir = settings.temp_dir();
        std::fs::create_dir_all(&temp_dir)?;

        Ok(Self {
            settings,
            prompts,
            transcriber,
            embedder,
            vector_store,
            temp_dir,
        })
    }

    /// Get a reference to the vector store (as trait object).
    pub fn vector_store(&self) -> Arc<dyn VectorStore> {
        self.vector_store.clone() as Arc<dyn VectorStore>
    }

    /// Get a reference to the SQLite vector store (for transcript storage).
    pub fn sqlite_store(&self) -> Arc<SqliteVectorStore> {
        self.vector_store.clone()
    }

    /// Get a reference to the embedder.
    pub fn embedder(&self) -> Arc<dyn Embedder> {
        self.embedder.clone()
    }

    /// Get the settings.
    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// Process media: download audio, transcribe, chunk, embed, and index.
    #[instrument(skip(self), fields(input = %input))]
    pub async fn process_media(&self, input: &str, force: bool) -> Result<ProcessResult> {
        // Parse input
        let (source, media_id) = parse_input(input).ok_or_else(|| {
            LyttError::InvalidInput(format!("Could not parse input: {}", input))
        })?;

        // Check if already indexed
        if !force && self.vector_store.is_video_indexed(&media_id).await? {
            info!("Media {} is already indexed, skipping", media_id);
            return Ok(ProcessResult {
                media_id,
                title: "Already indexed".to_string(),
                chunks_indexed: 0,
                skipped: true,
            });
        }

        // Fetch metadata
        info!("Fetching metadata for {}", media_id);
        eprintln!("  Fetching metadata...");
        let metadata = source.fetch_media(&media_id).await?;
        eprintln!("  Title: {}", metadata.title);

        // Check duration limit
        if let Some(duration) = metadata.duration_seconds {
            let mins = duration / 60;
            let secs = duration % 60;
            eprintln!("  Duration: {}:{:02}", mins, secs);
            if duration > self.settings.transcription.max_duration_seconds {
                return Err(LyttError::InvalidInput(format!(
                    "Media duration ({} seconds) exceeds maximum ({} seconds)",
                    duration, self.settings.transcription.max_duration_seconds
                )));
            }
        }

        // Download/extract audio
        info!("Extracting audio for: {}", metadata.title);
        eprintln!("  Downloading audio...");
        let audio_path = download_audio(&metadata.source_url, &media_id, &self.temp_dir).await?;
        eprintln!("  Audio downloaded.");

        // Transcribe
        info!("Transcribing audio...");
        eprintln!("  Transcribing...");
        let transcript = self.transcriber.transcribe(&audio_path).await?;
        eprintln!("  Transcription complete ({} segments)", transcript.segments.len());

        // Store raw transcript for potential rechunking
        if let Err(e) = self.vector_store.store_transcript(&media_id, &metadata.title, &transcript) {
            warn!("Failed to store transcript (rechunking won't be available): {}", e);
        }

        // Chunk
        info!("Chunking transcript...");
        eprintln!("  Chunking transcript...");
        let chunks = self.chunk_transcript(&transcript).await?;
        eprintln!("  Created {} chunks", chunks.len());

        // Index
        info!("Indexing {} chunks...", chunks.len());
        eprintln!("  Generating embeddings and indexing...");
        let indexed = self.index_chunks(&metadata, chunks).await?;
        eprintln!("  Indexed {} chunks", indexed);

        // Cleanup audio file
        if let Err(e) = std::fs::remove_file(&audio_path) {
            warn!("Failed to cleanup audio file: {}", e);
        }

        Ok(ProcessResult {
            media_id: metadata.id,
            title: metadata.title,
            chunks_indexed: indexed,
            skipped: false,
        })
    }

    /// Chunk a transcript.
    async fn chunk_transcript(&self, transcript: &Transcript) -> Result<Vec<ContentChunk>> {
        let strategy: ChunkingStrategy = self
            .settings
            .chunking
            .strategy
            .parse()
            .unwrap_or(ChunkingStrategy::Semantic);

        let chunker = create_chunker_with_prompts(strategy, self.prompts.clone());

        let config = ChunkingConfig {
            target_duration: self.settings.chunking.target_chunk_seconds,
            min_duration: self.settings.chunking.min_chunk_seconds,
            max_duration: self.settings.chunking.max_chunk_seconds,
        };

        chunker.chunk(transcript, &config).await
    }

    /// Generate embeddings and index chunks.
    async fn index_chunks(
        &self,
        metadata: &MediaMetadata,
        chunks: Vec<ContentChunk>,
    ) -> Result<usize> {
        if chunks.is_empty() {
            return Ok(0);
        }

        // Delete existing documents for this media
        self.vector_store.delete_by_video_id(&metadata.id).await?;

        // Generate embeddings in batch
        let texts: Vec<String> = chunks.iter().map(|c| c.content.clone()).collect();
        let embeddings = self.embedder.embed_batch(&texts).await?;

        // Create documents
        let documents: Vec<Document> = chunks
            .into_iter()
            .zip(embeddings)
            .map(|(chunk, embedding)| {
                Document::new(
                    metadata.id.clone(),
                    metadata.title.clone(),
                    chunk.title,
                    chunk.content,
                    chunk.start_seconds,
                    chunk.end_seconds,
                    embedding,
                    chunk.order,
                    metadata.published_at,
                )
            })
            .collect();

        // Batch upsert
        let count = self.vector_store.upsert_batch(&documents).await?;

        Ok(count)
    }

    /// Rechunk existing media (re-chunk and re-embed without re-transcribing).
    /// Requires the transcript to be stored in the database.
    #[instrument(skip(self))]
    pub async fn rechunk_media(&self, video_id: &str) -> Result<ProcessResult> {
        // Get stored transcript
        let (title, transcript) = self
            .vector_store
            .get_transcript(video_id)?
            .ok_or_else(|| {
                LyttError::InvalidInput(format!(
                    "No stored transcript for '{}'. Transcripts are only stored for videos \
                     transcribed after this feature was added. Use --force to re-transcribe.",
                    video_id
                ))
            })?;

        info!("Rechunking '{}' from stored transcript", title);

        // Chunk with current settings and prompts
        let strategy: ChunkingStrategy = self
            .settings
            .chunking
            .strategy
            .parse()
            .unwrap_or(ChunkingStrategy::Semantic);

        let chunker = create_chunker_with_prompts(strategy, self.prompts.clone());

        let config = ChunkingConfig {
            target_duration: self.settings.chunking.target_chunk_seconds,
            min_duration: self.settings.chunking.min_chunk_seconds,
            max_duration: self.settings.chunking.max_chunk_seconds,
        };

        let chunks = chunker.chunk(&transcript, &config).await?;

        // Delete old chunks
        self.vector_store.delete_by_video_id(video_id).await?;

        // Generate new embeddings
        let texts: Vec<String> = chunks.iter().map(|c| c.content.clone()).collect();
        let embeddings = self.embedder.embed_batch(&texts).await?;

        // Create documents
        let documents: Vec<Document> = chunks
            .into_iter()
            .zip(embeddings)
            .map(|(chunk, embedding)| {
                Document::new(
                    video_id.to_string(),
                    title.clone(),
                    chunk.title,
                    chunk.content,
                    chunk.start_seconds,
                    chunk.end_seconds,
                    embedding,
                    chunk.order,
                    None,
                )
            })
            .collect();

        // Index
        let count = self.vector_store.upsert_batch(&documents).await?;

        Ok(ProcessResult {
            media_id: video_id.to_string(),
            title,
            chunks_indexed: count,
            skipped: false,
        })
    }

    /// List all videos that have stored transcripts (available for rechunking).
    pub fn list_rechunkable(&self) -> Result<Vec<(String, String, f64)>> {
        self.vector_store.list_transcripts()
    }
}

/// Result of processing media.
#[derive(Debug)]
pub struct ProcessResult {
    /// Media ID.
    pub media_id: String,
    /// Title.
    pub title: String,
    /// Number of chunks indexed.
    pub chunks_indexed: usize,
    /// Whether processing was skipped (already indexed).
    pub skipped: bool,
}
