#!/bin/bash
# install.sh — build + register MailForge.app as the system mailto handler.
# Run once per Mac.
#
# Result: clicking any mailto: URL system-wide opens MailForge's compose
# form pre-filled (instead of Mail.app or whatever was registered before).
#
# Prereqs:
#   - MailForge daemon running on 127.0.0.1:8765 (com.williamnapier.mailforge launchd agent)
#   - duti (brew install duti) — used to set the system default; if absent,
#     register manually via System Settings → Default mail reader.

set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
APP="$HOME/Applications/MailForge.app"

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS"
cp "$HERE/Info.plist" "$APP/Contents/Info.plist"
cp "$HERE/mailforge-mailto.sh" "$APP/Contents/MacOS/mailforge-mailto"
chmod +x "$APP/Contents/MacOS/mailforge-mailto"

# Register with Launch Services
/System/Library/Frameworks/CoreServices.framework/Versions/A/Frameworks/LaunchServices.framework/Versions/A/Support/lsregister -f "$APP"

# Set as default mailto handler (requires duti)
if command -v duti >/dev/null; then
    duti -s com.williamnapier.mailforge-mailto mailto
    echo "Default mailto handler set to MailForge."
else
    echo "duti not installed. Install via: brew install duti"
    echo "Then run: duti -s com.williamnapier.mailforge-mailto mailto"
    echo "Or manually: System Settings → General → Default mail reader → MailForge"
fi

echo "Done. Test with: open 'mailto:test@example.com?subject=hello'"
