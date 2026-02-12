# email-extract

Extract and convert MIME email files (Maildir format) to plain text, markdown with YAML frontmatter, or JSON.

## Why

CLI email tools like himalaya give you raw MIME messages. This tool bridges the gap between raw email and William's markdown-based PKM system (Forge). It parses MIME structure, extracts the best body representation (preferring text/plain, falling back to HTML with intelligent tag stripping), and outputs clean structured data.

## Installation

```
cd ~/dotfiles/rust-projects/email-extract
cargo install --path .
cp ~/.cargo/bin/email-extract ~/.local/bin/
```

## Usage

```
# Single email, plain text (default)
email-extract /path/to/email.eml

# Markdown with YAML frontmatter (for archiving to Forge)
email-extract -f markdown /path/to/email.eml

# JSON output (for piping to nushell)
email-extract -f json /path/to/email.eml

# Process all emails in a Maildir
email-extract --maildir ~/Mail/

# Batch extract to a directory
email-extract --maildir ~/Mail/ -f markdown -o ~/Forge/captures/email/

# Metadata only (no body)
email-extract --metadata-only -f json /path/to/email.eml

# Limit to 10 most recent
email-extract --maildir ~/Mail/ -n 10 -f json

# Prefer HTML body (useful when plain text is truncated)
email-extract --prefer-html /path/to/email.eml

# Full headers (not just From/To/Date/Subject)
email-extract --full-headers /path/to/email.eml
```

## Output Formats

### Plain text (default)
```
From:    William Napier <william@example.com>
To:      Jane Smith <jane@example.com>
Date:    2026-02-12T09:30:00+00:00
Subject: Clinical handoff notes

Body content here...
```

### Markdown (for Forge integration)
```markdown
---
from: "William Napier <william@example.com>"
to: "Jane Smith <jane@example.com>"
date: "2026-02-12T09:30:00+00:00"
subject: "Clinical handoff notes"
body_type: "text/plain"
---

# Clinical handoff notes

Body content here...
```

### JSON (for Nushell piping)
```json
{
  "from": "William Napier <william@example.com>",
  "to": "Jane Smith <jane@example.com>",
  "date_parsed": "2026-02-12T09:30:00+00:00",
  "subject": "Clinical handoff notes",
  "body": "Body content here...",
  "body_type": "text/plain"
}
```

## How the Pieces Fit Together

```
Google Workspace
    |
    v
himalaya (IMAP/SMTP via OAuth2)  <-- reads/sends email
    |
    v
email-extract (MIME parsing)     <-- converts to text/markdown/JSON
    |
    v
Forge / DayPage / Nushell        <-- your PKM and workflow tools
```

Optional full-sync path:
```
Google Workspace
    |
    v
lieer/gmi (Gmail API sync)      <-- bidirectional tag sync
    |
    v
notmuch (search index)          <-- fast local search
    |
    v
~/Mail/ (Maildir)               <-- local email store
    |
    v
email-extract                   <-- same tool, processes local files
```

## Nushell Integration

After adding the wrappers to config.nu (already done), these commands are available:

```nushell
# List recent emails as a table
hmail-list | select id from subject date

# Search for clinical handoff emails
hmail-search "subject:clinical handoff"

# Read and extract to markdown
hmail-extract 42 -f markdown

# Pipe JSON through nushell for filtering
hmail-list | where from =~ "jane@"

# Notmuch full-text search (after lieer setup)
nm-search "from:jane AND date:2026-02"
```

## Manual OAuth Setup (Required)

himalaya cannot complete OAuth without a browser session. Follow these steps:

1. Go to https://console.cloud.google.com/
2. Create a new project (or use an existing one)
3. Enable the Gmail API: APIs & Services > Library > Gmail API > Enable
4. Create OAuth credentials: APIs & Services > Credentials > Create Credentials > OAuth client ID
   - Application type: Desktop app
   - Download the client ID and client secret
5. Edit `~/.config/himalaya/config.toml`:
   - Replace `REPLACE_WITH_YOUR_EMAIL@yourdomain.com` with your email
   - Replace `REPLACE_WITH_OAUTH_CLIENT_ID` with your client ID
6. Run: `himalaya account configure personal`
   - This opens a browser for Google OAuth consent
   - Tokens are stored securely in the system keyring
7. Test: `himalaya envelope list`

## Dependencies

- **mailparse**: MIME parsing (RFC 2822, multipart, encodings)
- **clap**: CLI argument parsing (derive macros)
- **chrono**: Date parsing and ISO 8601 formatting
- **serde/serde_json**: JSON serialisation
- **regex**: HTML tag stripping and entity decoding
- **anyhow**: Error handling
