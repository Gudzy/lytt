#!/bin/sh
# Authentication setup for yt-dlp.
#
# Priority:
#   1. OAuth2  — set YTDLP_OAUTH_TOKEN (base64-encoded JSON) to enable.
#                Persists for months; auto-refreshes on each use.
#   2. Cookies — set YTDLP_COOKIES (base64-encoded Netscape format) as fallback.
#                Requires manual rotation every ~4 weeks.
#
# lytt checks /tmp/yt-auth-oauth2 at request time to decide which method to use.

# ── OAuth2 (primary) ─────────────────────────────────────────────────────────
if [ -n "$YTDLP_OAUTH_TOKEN" ]; then
    python3 -c "
import os, base64, sys, json

raw = os.environ['YTDLP_OAUTH_TOKEN'].strip()
sys.stderr.write('[entrypoint] YTDLP_OAUTH_TOKEN length=%d\n' % len(raw))

try:
    data = base64.b64decode(raw).decode('utf-8')
    json.loads(data)  # validate JSON
except Exception as e:
    sys.stderr.write('[entrypoint] OAuth2 token decode FAILED: %s\n' % e)
    sys.exit(0)

import os as _os
_os.makedirs('/root/.cache/yt-dlp/youtube', exist_ok=True)
with open('/root/.cache/yt-dlp/youtube/oauth2_access_token.json', 'w') as f:
    f.write(data)

# Signal file: lytt checks this to decide which auth method to use.
open('/tmp/yt-auth-oauth2', 'w').close()
sys.stderr.write('[entrypoint] OAuth2 token loaded — using OAuth2 auth\n')
" 2>&1
fi

# ── Cookies (fallback) ────────────────────────────────────────────────────────
# Always set up cookie file. Even when OAuth2 is active, having cookies decoded
# costs nothing and lets you switch back without a redeploy.
if [ -n "$YTDLP_COOKIES" ]; then
    python3 -c "
import os, base64, sys

raw = os.environ['YTDLP_COOKIES'].strip()
sys.stderr.write('[entrypoint] YTDLP_COOKIES length=%d\n' % len(raw))

try:
    data = base64.b64decode(raw)
except Exception as e:
    sys.stderr.write('[entrypoint] base64 decode FAILED: %s\n' % e)
    open('/tmp/yt-cookies.txt', 'wb').close()
    sys.exit(0)

# Handle BOMs and encoding: convert everything to UTF-8 bytes
if data[:2] in (b'\xff\xfe', b'\xfe\xff'):
    sys.stderr.write('[entrypoint] detected UTF-16, converting to UTF-8\n')
    data = data.decode('utf-16').encode('utf-8')
elif data[:3] == b'\xef\xbb\xbf':
    sys.stderr.write('[entrypoint] detected UTF-8 BOM, stripping\n')
    data = data[3:]

# Normalize line endings to Unix
data = data.replace(b'\r\n', b'\n').replace(b'\r', b'\n')

with open('/tmp/yt-cookies.txt', 'wb') as f:
    f.write(data)

first = data.split(b'\n')[0].decode('utf-8', errors='replace')
sys.stderr.write('[entrypoint] cookies loaded, first line: %s\n' % first)
" 2>&1
else
    touch /tmp/yt-cookies.txt
    if [ ! -f /tmp/yt-auth-oauth2 ]; then
        echo '[entrypoint] WARNING: neither YTDLP_OAUTH_TOKEN nor YTDLP_COOKIES set — YouTube downloads may fail' >&2
    fi
fi

# ── bgutil PO token service ───────────────────────────────────────────────────
# Generates YouTube Proof-of-Origin tokens needed from cloud/datacenter IPs.
# Required regardless of authentication method.
echo '[entrypoint] Starting bgutil PO token service...' >&2
cd /bgutil-server && node build/main.js &
cd /data
sleep 3
echo '[entrypoint] bgutil started on port 4416' >&2

exec "$@"
