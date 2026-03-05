#!/bin/sh
# Write YouTube cookies from the YTDLP_COOKIES environment variable.
# If not set, create an empty file so yt-dlp doesn't error on the --cookies flag.
if [ -n "$YTDLP_COOKIES" ]; then
    python3 -c "
import os, base64, sys

raw = os.environ['YTDLP_COOKIES'].strip()
sys.stderr.write('[entrypoint] YTDLP_COOKIES present, length=%d\n' % len(raw))

try:
    data = base64.b64decode(raw)
    sys.stderr.write('[entrypoint] base64 decoded: %d bytes\n' % len(data))
except Exception as e:
    sys.stderr.write('[entrypoint] base64 decode FAILED: %s\n' % e)
    open('/tmp/yt-cookies.txt', 'w').close()
    sys.exit(0)

for enc in ('utf-8-sig', 'utf-16', 'latin-1'):
    try:
        text = data.decode(enc)
        # Normalize to Unix line endings
        text = text.replace('\r\n', '\n').replace('\r', '\n')
        with open('/tmp/yt-cookies.txt', 'w', encoding='utf-8') as f:
            f.write(text)
        lines = [l for l in text.splitlines() if l.strip() and not l.startswith('#')]
        sys.stderr.write('[entrypoint] cookies written (%s): %d cookie entries\n' % (enc, len(lines)))
        if lines:
            sys.stderr.write('[entrypoint] first cookie domain: %s\n' % lines[0].split('\t')[0])
        break
    except Exception as e:
        sys.stderr.write('[entrypoint] decode failed (%s): %s\n' % (enc, e))
" 2>&1
else
    touch /tmp/yt-cookies.txt
    echo '[entrypoint] YTDLP_COOKIES not set, cookie file is empty' >&2
fi

exec "$@"
