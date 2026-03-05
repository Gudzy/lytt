# ── Stage 1: Rust build ─────────────────────────────────────────────────────
FROM rust:1.83-bookworm AS builder

WORKDIR /app

COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release

# ── Stage 2: Minimal runtime ─────────────────────────────────────────────────
FROM debian:bookworm-slim

# yt-dlp requires Python 3 and ffmpeg. Node.js is required by bgutil-ytdlp-pot-provider
# to generate YouTube Proof-of-Origin tokens (needed for cloud IP bot detection bypass).
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    python3 \
    python3-pip \
    ffmpeg \
    nodejs \
    && pip3 install --no-cache-dir --upgrade \
        yt-dlp \
        yt-dlp-get-pot \
        bgutil-ytdlp-pot-provider \
        --break-system-packages \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/lytt /usr/local/bin/lytt

# Runtime data directory for SQLite vector store.
RUN mkdir -p /data /root/.config/lytt

# Bake in a minimal config. OPENAI_API_KEY is supplied as an env var at runtime.
COPY docker/config.toml /root/.config/lytt/config.toml

# Entrypoint: write YouTube cookies from env var before starting lytt.
COPY docker/entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh

WORKDIR /data

EXPOSE 8080

ENTRYPOINT ["/entrypoint.sh"]
CMD ["lytt", "serve", "--host", "0.0.0.0", "--port", "8080"]
