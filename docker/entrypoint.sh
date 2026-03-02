#!/bin/sh
# If YTDLP_OAUTH2_TOKEN is set, write it to the yt-dlp-plugin-oauth2 cache
# location so yt-dlp can authenticate with YouTube without bot detection.
if [ -n "$YTDLP_OAUTH2_TOKEN" ]; then
    mkdir -p /root/.cache/yt-dlp/youtube-oauth2
    printf '%s' "$YTDLP_OAUTH2_TOKEN" > /root/.cache/yt-dlp/youtube-oauth2/token_data.json
fi

exec "$@"
