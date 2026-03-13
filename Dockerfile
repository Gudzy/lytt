# ── Stage 1: Rust build ─────────────────────────────────────────────────────
FROM rust:1.83-bookworm AS builder

WORKDIR /app

COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release

# ── Stage 2: Minimal runtime ─────────────────────────────────────────────────
FROM debian:bookworm-slim

# yt-dlp requires Python 3 and ffmpeg.
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    python3 \
    python3-pip \
    ffmpeg \
    libatomic1 \
    && pip3 install --no-cache-dir --upgrade \
        yt-dlp \
        --break-system-packages \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/lytt /usr/local/bin/lytt

# Runtime data directory for SQLite vector store.
RUN mkdir -p /data /root/.config/lytt /root/.config/yt-dlp

# Bake in a minimal config. OPENAI_API_KEY is supplied as an env var at runtime.
COPY docker/config.toml /root/.config/lytt/config.toml

# yt-dlp config.
COPY docker/yt-dlp.conf /root/.config/yt-dlp/config

# Entrypoint: write YouTube cookies from env var before starting lytt.
COPY docker/entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh

WORKDIR /data

EXPOSE 8080

ENTRYPOINT ["/entrypoint.sh"]
CMD ["lytt", "serve", "--host", "0.0.0.0", "--port", "8080"]
