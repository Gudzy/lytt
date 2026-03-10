//! HTTP API server for integration with other systems.
//!
//! Provides REST endpoints for transcription, search, and RAG queries.

use crate::cli::Output;
use crate::config::Settings;
use crate::embedding::Embedder;
use crate::orchestrator::Orchestrator;
use crate::rag::RagEngine;
use async_openai::types::{
    ChatCompletionRequestUserMessageArgs, CreateChatCompletionRequestArgs,
};
use axum::{
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use crate::audio_source::parse_input;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use tower_http::cors::{Any, CorsLayer};

/// Shared application state.
struct AppState {
    orchestrator: Orchestrator,
    settings: Settings,
    /// Tracks media IDs currently being processed in the background.
    processing_jobs: Mutex<HashSet<String>>,
    /// Tracks media IDs whose background processing failed, with the error message.
    failed_jobs: Mutex<HashMap<String, String>>,
}

/// Run the HTTP API server.
pub async fn run_serve(host: &str, port: u16, settings: Settings) -> anyhow::Result<()> {
    let orchestrator = Orchestrator::new(settings.clone())?;

    let state = Arc::new(AppState {
        orchestrator,
        settings,
        processing_jobs: Mutex::new(HashSet::new()),
        failed_jobs: Mutex::new(HashMap::new()),
    });

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let app = Router::new()
        .route("/health", get(health))
        .route("/transcribe", post(transcribe))
        .route("/search", post(search))
        .route("/ask", post(ask))
        .route("/media", get(list_media))
        .route("/media/{video_id}", get(get_media))
        .route("/media/{video_id}/summary", get(get_media_summary))
        .layer(cors)
        .with_state(state);

    let addr = format!("{}:{}", host, port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;

    Output::header("Lytt API Server");
    println!();
    Output::success(&format!("Listening on http://{}", addr));
    println!();
    println!("Endpoints:");
    Output::kv("Health", "GET  /health");
    Output::kv("Transcribe", "POST /transcribe");
    Output::kv("Search", "POST /search");
    Output::kv("Ask (RAG)", "POST /ask");
    Output::kv("List Media", "GET  /media");
    Output::kv("Get Media", "GET  /media/:video_id");
    println!();
    Output::info("Press Ctrl+C to stop the server.");

    axum::serve(listener, app).await?;

    Ok(())
}

// === Request/Response Types ===

#[derive(Deserialize)]
struct TranscribeRequest {
    /// YouTube URL/ID or local file path
    input: String,
    /// Force re-processing even if already indexed
    #[serde(default)]
    force: bool,
}

#[derive(Serialize)]
struct TranscribeResponse {
    success: bool,
    media_id: String,
    title: String,
    chunks_indexed: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Deserialize)]
struct SearchRequest {
    query: String,
    #[serde(default = "default_limit")]
    limit: usize,
    #[serde(default = "default_min_score")]
    min_score: f32,
}

fn default_limit() -> usize {
    5
}

fn default_min_score() -> f32 {
    0.3
}

#[derive(Serialize)]
struct SearchResponse {
    results: Vec<SearchResult>,
}

#[derive(Serialize)]
struct SearchResult {
    video_id: String,
    video_title: String,
    chunk_title: String,
    content: String,
    start_seconds: f64,
    end_seconds: f64,
    timestamp: String,
    score: f32,
}

#[derive(Deserialize)]
struct AskRequest {
    question: String,
    #[serde(default = "default_max_chunks")]
    max_chunks: usize,
    #[serde(default)]
    model: Option<String>,
}

fn default_max_chunks() -> usize {
    10
}

#[derive(Serialize)]
struct AskResponse {
    answer: String,
    sources: Vec<SourceInfo>,
}

#[derive(Serialize)]
struct SourceInfo {
    video_id: String,
    video_title: String,
    timestamp: String,
    score: f32,
    content: String,
}

#[derive(Serialize)]
struct MediaListResponse {
    media: Vec<MediaInfo>,
    total: usize,
}

#[derive(Serialize)]
struct MediaInfo {
    video_id: String,
    video_title: String,
    chunk_count: u32,
    total_duration_seconds: f64,
}

#[derive(Serialize)]
struct MediaDetailResponse {
    video_id: String,
    video_title: String,
    chunk_count: usize,
    total_duration_seconds: f64,
    chunks: Vec<ChunkInfo>,
}

#[derive(Serialize)]
struct ChunkInfo {
    title: String,
    content: String,
    start_seconds: f64,
    end_seconds: f64,
}

#[derive(Serialize)]
struct ErrorResponse {
    error: String,
}

#[derive(Serialize)]
struct SummaryResponse {
    summary: String,
}

// === Handlers ===

async fn health() -> impl IntoResponse {
    Json(serde_json::json!({ "status": "ok" }))
}

async fn transcribe(
    State(state): State<Arc<AppState>>,
    Json(req): Json<TranscribeRequest>,
) -> impl IntoResponse {
    // Extract media ID without downloading.
    let media_id = match parse_input(&req.input) {
        Some((_, id)) => id,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(TranscribeResponse {
                    success: false,
                    media_id: String::new(),
                    title: String::new(),
                    chunks_indexed: 0,
                    error: Some("Unsupported input format".to_string()),
                }),
            )
                .into_response()
        }
    };

    // Fast path: already indexed in the vector store — return immediately.
    if !req.force {
        if let Ok(chunks) = state.orchestrator.vector_store().get_by_video_id(&media_id).await {
            if !chunks.is_empty() {
                let title = chunks.first().map(|c| c.video_title.clone()).unwrap_or_default();
                return Json(TranscribeResponse {
                    success: true,
                    media_id,
                    title,
                    chunks_indexed: chunks.len(),
                    error: None,
                })
                .into_response();
            }
        }
    }

    // Atomic check-and-insert: if already in-flight return 202; otherwise claim the slot.
    // A single lock acquisition prevents duplicate background tasks for the same media ID.
    let already_processing = {
        let mut jobs = state.processing_jobs.lock().unwrap();
        if jobs.contains(&media_id) {
            true
        } else {
            jobs.insert(media_id.clone());
            false
        }
    };

    if already_processing {
        return (
            StatusCode::ACCEPTED,
            Json(TranscribeResponse {
                success: true,
                media_id,
                title: String::new(),
                chunks_indexed: 0,
                error: None,
            }),
        )
            .into_response();
    }

    // Kick off background processing and return 202 immediately.
    let state_clone = Arc::clone(&state);
    let input = req.input.clone();
    let force = req.force;
    let media_id_bg = media_id.clone();
    tokio::spawn(async move {
        let result = state_clone.orchestrator.process_media(&input, force).await;
        {
            let mut jobs = state_clone.processing_jobs.lock().unwrap();
            jobs.remove(&media_id_bg);
        }
        match result {
            Ok(_) => {
                // Clear any prior failure entry on successful (re-)processing.
                state_clone.failed_jobs.lock().unwrap().remove(&media_id_bg);
            }
            Err(e) => {
                tracing::error!("Background transcription failed for {}: {}", media_id_bg, e);
                state_clone.failed_jobs.lock().unwrap().insert(media_id_bg, e.to_string());
            }
        }
    });

    (
        StatusCode::ACCEPTED,
        Json(TranscribeResponse {
            success: true,
            media_id,
            title: String::new(),
            chunks_indexed: 0,
            error: None,
        }),
    )
        .into_response()
}

async fn search(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SearchRequest>,
) -> impl IntoResponse {
    // Reuse the shared embedder from the orchestrator (avoids creating a new HTTP client per request).
    let query_embedding = match state.orchestrator.embedder().embed(&req.query).await {
        Ok(emb) => emb,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
                .into_response()
        }
    };

    // Search vector store
    match state
        .orchestrator
        .vector_store()
        .search_with_threshold(&query_embedding, req.limit, req.min_score)
        .await
    {
        Ok(results) => Json(SearchResponse {
            results: results
                .into_iter()
                .map(|r| {
                    let timestamp = r.document.format_timestamp();
                    SearchResult {
                        video_id: r.document.video_id,
                        video_title: r.document.video_title,
                        chunk_title: r.document.section_title.unwrap_or_default(),
                        content: r.document.content,
                        start_seconds: r.document.start_seconds,
                        end_seconds: r.document.end_seconds,
                        timestamp,
                        score: r.score,
                    }
                })
                .collect(),
        })
        .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
            .into_response(),
    }
}

async fn ask(
    State(state): State<Arc<AppState>>,
    Json(req): Json<AskRequest>,
) -> impl IntoResponse {
    let model = req
        .model
        .unwrap_or_else(|| state.settings.rag.model.clone());

    // Reuse the shared embedder from the orchestrator (avoids creating a new HTTP client per request).
    let engine = RagEngine::new(
        state.orchestrator.vector_store(),
        state.orchestrator.embedder(),
        &model,
        req.max_chunks,
    );

    match engine.ask(&req.question).await {
        Ok(response) => Json(AskResponse {
            answer: response.answer,
            sources: response
                .sources
                .into_iter()
                .map(|s| SourceInfo {
                    video_id: s.video_id,
                    video_title: s.video_title,
                    timestamp: s.timestamp,
                    score: s.score,
                    content: s.content,
                })
                .collect(),
        })
        .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
            .into_response(),
    }
}

async fn list_media(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match state.orchestrator.vector_store().list_videos().await {
        Ok(media) => Json(MediaListResponse {
            total: media.len(),
            media: media
                .into_iter()
                .map(|m| MediaInfo {
                    video_id: m.video_id,
                    video_title: m.video_title,
                    chunk_count: m.chunk_count,
                    total_duration_seconds: m.total_duration_seconds,
                })
                .collect(),
        })
        .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
            .into_response(),
    }
}

async fn get_media(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(video_id): axum::extract::Path<String>,
) -> impl IntoResponse {
    // If background processing failed, return 500 so pollers can distinguish it from
    // "still processing" (404) and surface a meaningful error to the client.
    {
        let failed = state.failed_jobs.lock().unwrap();
        if let Some(error) = failed.get(&video_id) {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse { error: error.clone() }),
            )
                .into_response();
        }
    }

    match state
        .orchestrator
        .vector_store()
        .get_by_video_id(&video_id)
        .await
    {
        Ok(chunks) if chunks.is_empty() => (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: format!("Media not found: {}", video_id),
            }),
        )
            .into_response(),
        Ok(chunks) => {
            // Chunks are already ORDER BY chunk_order from the SQLite query.

            let video_title = chunks.first().map(|c| c.video_title.clone()).unwrap_or_default();
            let total_duration = chunks
                .iter()
                .map(|c| c.end_seconds)
                .fold(0.0f64, |a, b| a.max(b));

            Json(MediaDetailResponse {
                video_id,
                video_title,
                chunk_count: chunks.len(),
                total_duration_seconds: total_duration,
                chunks: chunks
                    .into_iter()
                    .map(|c| ChunkInfo {
                        title: c.section_title.unwrap_or_default(),
                        content: c.content,
                        start_seconds: c.start_seconds,
                        end_seconds: c.end_seconds,
                    })
                    .collect(),
            })
            .into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
            .into_response(),
    }
}

async fn get_media_summary(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(video_id): axum::extract::Path<String>,
) -> impl IntoResponse {
    let store = state.orchestrator.sqlite_store();

    // Return cached summary immediately if available.
    match store.get_summary(&video_id) {
        Ok(Some(summary)) => return Json(SummaryResponse { summary }).into_response(),
        Ok(None) => {}
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse { error: e.to_string() }),
            )
                .into_response()
        }
    }

    // Fetch chunks — they are ORDER BY chunk_order from the SQLite query.
    let chunks = match state.orchestrator.vector_store().get_by_video_id(&video_id).await {
        Ok(c) if c.is_empty() => {
            return (
                StatusCode::NOT_FOUND,
                Json(ErrorResponse { error: format!("Media not found: {}", video_id) }),
            )
                .into_response()
        }
        Ok(c) => c,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse { error: e.to_string() }),
            )
                .into_response()
        }
    };

    let title = chunks.first().map(|c| c.video_title.as_str()).unwrap_or("Unknown");
    let transcript: String = chunks.iter().map(|c| c.content.as_str()).collect::<Vec<_>>().join(" ");

    let prompt = format!(
        "Summarize the following YouTube video transcript titled \"{title}\".\n\
         Write 3–5 concise bullet points covering the main topics, key insights, and \
         conclusions. Use • for bullet points. Be specific.\n\nTranscript:\n{transcript}",
        title = title,
        transcript = transcript,
    );

    let client = crate::openai::create_client();
    let request = match CreateChatCompletionRequestArgs::default()
        .model(&state.settings.rag.model)
        .messages(vec![
            ChatCompletionRequestUserMessageArgs::default()
                .content(prompt)
                .build()
                .unwrap()
                .into(),
        ])
        .temperature(0.3_f32)
        .build()
    {
        Ok(r) => r,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse { error: e.to_string() }),
            )
                .into_response()
        }
    };

    let response = match client.chat().create(request).await {
        Ok(r) => r,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse { error: format!("OpenAI error: {}", e) }),
            )
                .into_response()
        }
    };

    let summary = match response.choices.first().and_then(|c| c.message.content.as_ref()) {
        Some(s) => s.clone(),
        None => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse { error: "Empty response from OpenAI".to_string() }),
            )
                .into_response()
        }
    };

    // Cache for future requests (failure is non-fatal).
    if let Err(e) = store.store_summary(&video_id, &summary) {
        tracing::warn!("Failed to cache summary for {}: {}", video_id, e);
    }

    Json(SummaryResponse { summary }).into_response()
}
