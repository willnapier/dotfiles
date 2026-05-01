#!/bin/bash
# MailForge mailto: handler. Receives the URL as $1, parses it,
# redirects to MailForge's compose form. Requires the mailforge
# daemon running on 127.0.0.1:8765 (managed by launchd agent
# com.williamnapier.mailforge).
#
# When clicked from anywhere on macOS (Mail.app preview, Safari,
# embedded mailto: in PDFs, etc.), this opens the user's default
# browser at MailForge's pre-filled compose form.

url="$1"
if [ -z "$url" ]; then
  exit 0
fi

# Log invocations for debugging
mkdir -p ~/Library/Logs
echo "$(date '+%Y-%m-%d %H:%M:%S') $url" >> ~/Library/Logs/mailforge-mailto.log

compose_url=$(/usr/bin/python3 - <<PY "$url"
import sys, urllib.parse as up
raw = sys.argv[1]
if not raw.lower().startswith('mailto:'):
    sys.exit(0)
rest = raw[7:]
qidx = rest.find('?')
if qidx >= 0:
    to_part = rest[:qidx]
    qs = rest[qidx+1:]
else:
    to_part = rest
    qs = ''
to = up.unquote(to_part)
params = up.parse_qs(qs, keep_blank_values=True)
out = [('to', to)]
for k, v in params.items():
    lk = k.lower()
    if lk in ('subject', 'body', 'cc', 'bcc') and v:
        out.append((lk, v[0]))
print('http://127.0.0.1:8765/mail/compose?' + up.urlencode(out))
PY
)

if [ -n "$compose_url" ]; then
  /usr/bin/open "$compose_url"
fi
