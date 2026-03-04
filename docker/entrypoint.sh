#!/bin/sh
# Write YouTube cookies from the YTDLP_COOKIES environment variable.
# If not set, create an empty file so yt-dlp doesn't error on the --cookies flag.
if [ -n "$YTDLP_COOKIES" ]; then
    python3 -c "
import os, base64
data = base64.b64decode(os.environ['YTDLP_COOKIES'].strip())
for enc in ('utf-8-sig', 'utf-16', 'latin-1'):
    try:
        text = data.decode(enc)
        with open('/tmp/yt-cookies.txt', 'w', encoding='utf-8') as f:
            f.write(text)
        break
    except Exception:
        continue
"
else
    touch /tmp/yt-cookies.txt
fi

exec "$@"
