# ── Stage 1: Rust build ─────────────────────────────────────────────────────
FROM rust:1.82-bookworm AS builder

WORKDIR /app

# Cache dependency compilation separately from source code.
# Only re-runs when Cargo.toml or Cargo.lock change.
COPY Cargo.toml Cargo.lock ./
RUN mkdir -p src && \
    printf 'fn main() {}\n' > src/main.rs && \
    printf 'pub fn placeholder() {}\n' > src/lib.rs && \
    cargo build --release && \
    rm -rf target/release/.fingerprint/lytt-*

# Build the real source.
COPY src ./src
RUN touch src/main.rs src/lib.rs && cargo build --release

# ── Stage 2: Minimal runtime ─────────────────────────────────────────────────
FROM debian:bookworm-slim

# yt-dlp requires Python 3. ffmpeg is required by yt-dlp for audio extraction.
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    python3 \
    python3-pip \
    ffmpeg \
    && pip3 install yt-dlp --break-system-packages \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/lytt /usr/local/bin/lytt

# Runtime data directory for SQLite vector store.
RUN mkdir -p /data /root/.config/lytt

# Bake in a minimal config. OPENAI_API_KEY is supplied as an env var at runtime.
COPY docker/config.toml /root/.config/lytt/config.toml

WORKDIR /data

EXPOSE 8080

CMD ["lytt", "serve", "--host", "0.0.0.0", "--port", "8080"]
