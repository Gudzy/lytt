#!/bin/sh
# Write YouTube cookies from the YTDLP_COOKIES environment variable.
# If not set, create an empty file so yt-dlp doesn't error on the --cookies flag.
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
sys.stderr.write('[entrypoint] first line: %s\n' % first)
" 2>&1
else
    touch /tmp/yt-cookies.txt
    echo '[entrypoint] YTDLP_COOKIES not set, cookie file is empty' >&2
fi

exec "$@"
