//! Lytt - Audio Transcription and RAG
//!
//! A local-first CLI tool for transcribing audio and building a searchable knowledge base.
//!
//! The name "Lytt" comes from the Norwegian/Scandinavian word for "listen."
//!
//! # Overview
//!
//! Lytt allows you to:
//! - Transcribe YouTube videos and local audio/video files
//! - Build a searchable vector database from audio content
//! - Ask questions and get AI-powered answers with citations
//! - Search through your audio library semantically
//!
//! # Architecture
//!
//! The library is organized into several modules:
//!
//! - `config` - Configuration management
//! - `audio_source` - Audio source abstraction (YouTube, local files)
//! - `audio` - Audio download and processing
//! - `transcription` - Speech-to-text transcription
//! - `chunking` - Content chunking strategies
//! - `embedding` - Embedding generation
//! - `vector_store` - Vector database abstraction
//! - `rag` - RAG engine for question answering
//! - `orchestrator` - Pipeline coordination
//!
//! # Example
//!
//! ```rust,no_run
//! use lytt::config::Settings;
//! use lytt::orchestrator::Orchestrator;
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     let settings = Settings::load()?;
//!     let orchestrator = Orchestrator::new(settings)?;
//!
//!     // Process audio from a YouTube video
//!     let result = orchestrator.process_media("dQw4w9WgXcQ", false).await?;
//!     println!("Indexed {} chunks", result.chunks_indexed);
//!
//!     Ok(())
//! }
//! ```

pub mod agent;
pub mod audio;
pub mod audio_source;
pub mod chunking;
pub mod cli;
pub mod config;
pub mod embedding;
pub mod error;
pub mod mcp;
pub mod openai;
pub mod orchestrator;
pub mod rag;
pub mod transcription;
pub mod util;
pub mod vector_store;

pub use error::{Result, LyttError};
