#!/bin/sh
# Write YouTube cookies from the YTDLP_COOKIES environment variable.
# If not set, create an empty file so yt-dlp doesn't error on the --cookies flag.
if [ -n "$YTDLP_COOKIES" ]; then
    printf '%s' "$YTDLP_COOKIES" | base64 -d > /tmp/yt-cookies.txt
else
    touch /tmp/yt-cookies.txt
fi

exec "$@"
