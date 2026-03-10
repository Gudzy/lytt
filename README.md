# Lytt

> A local-first CLI for audio transcription and RAG, deployed as an Azure Container App with automated YouTube authentication.

Forked from [smebbs/lytt](https://github.com/smebbs/lytt). The original project is a Rust CLI for transcribing audio, building a searchable vector knowledge base, and answering questions via RAG. This fork adds production Azure deployment with automated handling of YouTube's bot detection and cookie-based authentication.

---

## What this fork adds

| Feature | Description |
|---|---|
| **Azure Container App** | HTTP API served from `lytt serve`, deployed to Azure via GitHub Actions |
| **YouTube bot detection bypass** | bgutil PO token server + yt-dlp-ejs + authenticated cookies |
| **Automated cookie rotation** | Weekly Container App Job using Camoufox (hardened Firefox) refreshes cookies without human intervention |
| **Docker image** | Multi-stage build: Rust builder + Debian slim runtime with Python, ffmpeg, yt-dlp, Node.js |

---

## Architecture

```
GitHub push to master
        │
        ▼
GitHub Actions
  ├─ Build Rust binary
  ├─ Push image to Azure Container Registry (lyttacr.azurecr.io/lytt)
  └─ Deploy new revision to Container App
        │
        ▼
Azure Container App: lytt
  ├─ lytt serve --host 0.0.0.0 --port 8080
  ├─ entrypoint.sh: decode YTDLP_COOKIES → /tmp/yt-cookies.txt
  │                 start bgutil PO token server (port 4416)
  └─ yt-dlp → ffmpeg → Whisper API → OpenAI embeddings → SQLite
        │
        ▼ weekly (Mon 03:00 UTC)
Azure Container App Job: lytt-cookie-rotator
  ├─ Camoufox (hardened Firefox, headless)
  ├─ Seeds cookies from Azure Files, visits youtube.com
  ├─ Collects refreshed cookies
  └─ Updates YTDLP_COOKIES secret via Azure Management API (Managed Identity)
```

---

## YouTube Bot Detection

YouTube blocks yt-dlp from cloud datacenter IPs using multiple layers of detection. This deployment works around all of them:

### Problem 1: Proof-of-Origin (PO) tokens

YouTube requires a `pot` parameter signed with browser fingerprint data. yt-dlp cannot generate this alone.

**Solution:** [bgutil-ytdlp-pot-provider](https://github.com/brainicism/bgutil-ytdlp-pot-provider) — a Node.js server that generates valid PO tokens. Runs as a background process in `entrypoint.sh` on port 4416. The [yt-dlp-get-pot](https://github.com/coletdjnz/yt-dlp-get-pot) plugin routes PO token requests to it automatically.

**Pinned versions (required):**
- `bgutil-ytdlp-pot-provider==1.3.0`
- `yt-dlp-get-pot<0.3.0`

Version 1.3.1+ of bgutil changed the GetPOT interface in a way that breaks 0.2.x compatibility. Do not upgrade without testing.

### Problem 2: SABR / JS challenges

YouTube serves some responses as encrypted JavaScript challenges (SABR format). yt-dlp needs a JS runtime to solve them.

**Solution:** [yt-dlp-ejs](https://github.com/coletdjnz/yt-dlp-ejs) plugin with Node.js explicitly enabled:

```
# docker/yt-dlp.conf
--js-runtimes node:/usr/local/bin/node
```

yt-dlp defaults to Deno; Node.js must be explicitly declared.

### Problem 3: Cookie authentication

Many YouTube videos are unavailable without authentication from a browser-like session.

**Solution:** Export cookies from a logged-in browser session, base64-encode them, store as the `ytdlp-cookies` Azure Container App secret. `entrypoint.sh` decodes to `/tmp/yt-cookies.txt` at startup. All yt-dlp calls reference this file.

The `player_client=web` option in yt-dlp calls ensures the cookie-authenticated code path is used (other clients trigger more aggressive bot checks).

---

## Automated Cookie Rotation

YouTube cookie sessions last 2–4 weeks. The `lytt-cookie-rotator` Container App Job eliminates manual cookie refresh entirely.

### How it works

1. **Bootstrap (once):** Reads `YTDLP_COOKIES` env var, seeds cookies into Camoufox, visits YouTube, saves refreshed cookies to Azure Files (`/mnt/profile/cookies.txt`)
2. **Weekly rotation:** Loads `cookies.txt` from Azure Files, seeds into fresh Camoufox context, visits YouTube (renewing cookie TTL server-side), saves new cookies, updates the `ytdlp-cookies` secret in the `lytt` Container App via Azure Management API

The job uses a fresh browser context on every run — not a persistent Firefox profile. Firefox's SQLite databases (cookies.sqlite, places.sqlite) hang for 3+ minutes when accessed over Azure Files (SMB). A single `cookies.txt` text file on Azure Files works without issue.

### Safety guard

If the collected cookie set does not contain any of `SID`, `HSID`, `SSID`, `SAPISID`, `__Secure-1PSID`, or `__Secure-3PSID` — the hallmarks of an authenticated Google session — the job aborts without touching the existing secret. This prevents anonymous cookies from silently overwriting a working session.

### Infrastructure

| Component | Detail |
|---|---|
| Image | `lyttacr.azurecr.io/lytt-cookie-rotator:latest` |
| Schedule | `0 3 * * 1` (every Monday, 03:00 UTC) |
| Auth | System-assigned Managed Identity with Contributor on `dyngeseth-rg` |
| State | Azure Files share mounted at `/mnt/profile` (cookies.txt only) |
| Cost | < $0.10/month (Azure Files 1 GiB + ~3 min Consumption compute/week) |

### Bootstrap a new deployment

```bash
az containerapp job start \
  --name lytt-cookie-rotator \
  --resource-group dyngeseth-rg

az containerapp job execution logs show \
  --name lytt-cookie-rotator \
  --resource-group dyngeseth-rg \
  --execution-name $(az containerapp job execution list \
    --name lytt-cookie-rotator --resource-group dyngeseth-rg \
    --query "[0].name" -o tsv) \
  --follow
```

After a successful bootstrap run, the `YTDLP_COOKIES` env var on the job is no longer needed — the job is self-sustaining from `cookies.txt`.

---

## CI/CD

Two GitHub Actions workflows:

| Workflow | Trigger | Action |
|---|---|---|
| `docker.yml` | Push to `master` | Build Rust image, push to ACR, deploy new Container App revision |
| `cookie-rotator.yml` | Push to `master` touching `docker/cookie-rotator/**` | Build Python image, push to ACR |

Required secrets: `ACR_LOGIN_SERVER`, `ACR_USERNAME`, `ACR_PASSWORD`, `AZURE_CREDENTIALS`.

---

## Docker image

`Dockerfile` (root) — the main lytt service image:

```
rust:1.83-bookworm        → cargo build --release
debian:bookworm-slim      → runtime
  + python3, pip, ffmpeg
  + Node.js (LTS)
  + yt-dlp + bgutil-ytdlp-pot-provider==1.3.0
  + yt-dlp-get-pot<0.3.0 + yt-dlp-ejs
  + bgutil Node.js server (FROM brainicism/bgutil-ytdlp-pot-provider:node)
```

`docker/entrypoint.sh` runs at container start:
1. Decodes `YTDLP_COOKIES` (base64 Netscape format) to `/tmp/yt-cookies.txt`
2. Starts bgutil PO token server in background (`node build/main.js`)
3. Waits 3 seconds, then `exec`s the main process

`docker/config.toml` is baked into the image at `/root/.config/lytt/config.toml`:
- Whisper transcription, `text-embedding-3-small` embeddings
- Temporal chunking, SQLite vector store at `/data/lytt.db`
- GPT-4o-mini for RAG responses

---

## Configuration

`docker/config.toml` (baked into image):

```toml
[general]
data_dir = "/data"
log_level = "info"

[transcription]
provider = "whisper"
model = "whisper-1"

[embedding]
model = "text-embedding-3-small"
dimensions = 1536

[chunking]
strategy = "temporal"

[vector_store]
path = "/data/lytt.db"

[rag]
enabled = true
model = "gpt-4o-mini"
max_context_chunks = 10
```

Runtime environment variables (set as Container App secrets/settings):

| Variable | Description |
|---|---|
| `OPENAI_API_KEY` | OpenAI API key for Whisper + embeddings + RAG |
| `YTDLP_COOKIES` | Base64-encoded Netscape cookie file for YouTube authentication |

---

## HTTP API

`lytt serve --host 0.0.0.0 --port 8080` starts the REST API.

| Endpoint | Method | Description |
|---|---|---|
| `/health` | GET | Health check |
| `/transcribe` | POST | Transcribe and index a YouTube URL |
| `/status/:id` | GET | Poll transcription job status |
| `/search` | POST | Semantic search over indexed content |
| `/ask` | POST | RAG question answering |

---

## Local Development

### Prerequisites

- Rust 1.75+
- yt-dlp
- ffmpeg
- OpenAI API key

### Build

```bash
git clone https://github.com/Gudzy/lytt.git
cd lytt
cargo build --release
```

### Run CLI

```bash
export OPENAI_API_KEY="..."

# Transcribe a YouTube video
lytt transcribe https://youtube.com/watch?v=VIDEO_ID

# Ask a question about indexed content
lytt ask "What topics are covered?"

# Start HTTP API server
lytt serve
```

### Run with Docker

```bash
docker build -t lytt .
docker run -p 8080:8080 \
  -e OPENAI_API_KEY="..." \
  -e YTDLP_COOKIES="$(base64 -w0 cookies.txt)" \
  lytt
```

---

## CLI Reference

### `lytt transcribe <input>`

```
Options:
  -f, --force         Force re-processing even if already indexed
  --playlist          Treat input as playlist/channel URL
  --limit N           Max videos from playlist (default: 50)
  -o, --output FILE   Export transcript to file instead of indexing
  --format FORMAT     Output format: json, srt, vtt (default: json)
```

Supported inputs: YouTube URLs, video IDs, playlist/channel URLs, local audio/video files.

### `lytt ask <question>`

```
Options:
  -m, --model MODEL     LLM model (default: gpt-4o-mini)
  -c, --max-chunks N    Context chunks to include (default: 10)
```

### `lytt search <query>`

```
Options:
  -l, --limit N         Max results (default: 5)
  -m, --min-score N     Min similarity score 0.0–1.0 (default: 0.3)
```

### `lytt serve`

```
Options:
  --host HOST   Bind host (default: 127.0.0.1)
  -p, --port N  Port (default: 3000)
```

### Other commands

```bash
lytt list            # List all indexed media
lytt rechunk all     # Re-chunk with updated settings (no re-transcription)
lytt chat            # Interactive chat with knowledge base
lytt mcp             # Start MCP server for Claude Desktop/Code
lytt config show     # Display configuration
lytt doctor          # Verify requirements
```

---

## License

MIT — see [LICENSE](LICENSE).

Original project by [smebbs](https://github.com/smebbs/lytt).
