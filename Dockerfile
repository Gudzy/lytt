# ── Stage 0: bgutil PO token server ─────────────────────────────────────────
# Copies the pre-built bgutil server (Node.js + compiled JS) from the official image.
FROM brainicism/bgutil-ytdlp-pot-provider:node AS bgutil-server

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
        "yt-dlp-get-pot<0.3.0" \
        bgutil-ytdlp-pot-provider \
        --break-system-packages \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/lytt /usr/local/bin/lytt

# Copy Node.js binary and bgutil server from the official bgutil image.
# bgutil generates YouTube Proof-of-Origin tokens needed from cloud/datacenter IPs.
COPY --from=bgutil-server /usr/local/bin/node /usr/local/bin/node
COPY --from=bgutil-server /app /bgutil-server

# yt-dlp and GetPOT look for 'nodejs' (Debian package name) to detect the JS runtime.
RUN ln -s /usr/local/bin/node /usr/local/bin/nodejs

# bgutil-ytdlp-pot-provider's script-node provider looks for scripts at this path.
# Symlink our /bgutil-server to the expected location.
RUN mkdir -p /root/bgutil-ytdlp-pot-provider && \
    ln -s /bgutil-server /root/bgutil-ytdlp-pot-provider/server

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
