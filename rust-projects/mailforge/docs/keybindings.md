# mailforge keybindings spec

This is the **authoritative spec** for the keyboard JS implementation
agent. It maps William's existing meli keybindings (verified against
`~/.config/meli/config.toml`, last modified 2026-04-29) to the
equivalent action in mailforge's browser context.

## Layout assumption

William uses **Colemak-DH** with **neio** as the directional axis:

```
n e i o
| | | |
v v v v
LEFT DOWN UP RIGHT  (Colemak physical positions of vim's h j k l)
```

Implementation: detect via the literal `event.key` — the JS event reports
the **letter the user typed**, not the physical position. So `event.key`
is `"e"` for down, `"i"` for up, `"o"` for right, `"n"` for left, regardless
of the OS keyboard layout. The browser's keyboard event API normalises this
correctly across Wayland (Niri), macOS, and Windows; no layout sniffing
needed.

## Context model

Every page's `<body>` carries a `data-context` attribute set by
`templates.rs::page()`. Five contexts:

| `data-context` | Meaning |
|---|---|
| `listing` | Mailbox table (`/mail/<account>/<mailbox>`) |
| `message` | Single-message read view (`/mail/m/<id>`) |
| `thread` | Thread view (`/mail/t/<thread-id>`) |
| `compose` | Composer form (`/mail/compose`) |
| `search` | Search results (`/mail/search`) |

The JS dispatch table switches per context. Bindings that share semantics
across contexts (e.g. `j`/`k` for nav, `?` for help) live in a base
table that all contexts inherit.

## Focus rules

**Bare letter keys never fire when focus is in `<input>` or `<textarea>`.**
The handler checks `document.activeElement.tagName` and bails on `INPUT` /
`TEXTAREA` / `SELECT` for non-modified keys.

**Modified shortcuts (Ctrl+/Cmd+/Alt+) always fire**, regardless of
focus. So `Ctrl+Enter` in the composer textarea sends; `Escape` always
cancels modal state; `Ctrl+E` in the composer textarea opens Helix.

**Special exception: the `/` key in listing context** focuses the search
input. Once focused, `/` falls through to the input as a literal `/`
character (since the bareness check now sees `INPUT` as activeElement).

## Base bindings (all contexts unless overridden)

| Key | Action |
|---|---|
| `?` | Toggle a help overlay (modal listing all bindings for the current context). |
| `Escape` | Close modal / unfocus input / cancel current operation. |
| `Backspace` | Browser back (`history.back()`). The browser's default Backspace-back behaviour is gone in modern browsers; we restore it explicitly. |

## Listing (`/mail/<account>/<mailbox>`)

Mirrors William's meli `[shortcuts.listing]` and `[shortcuts.general]`.

| Key | Action | Maps from meli |
|---|---|---|
| `e` | Highlight next row (down) | `scroll_down = 'e'` (general) |
| `i` | Highlight previous row (up) | `scroll_up = 'i'` (general) |
| `n` | Sidebar/horizontal scroll left | `scroll_left = 'n'` (general) |
| `o` | Sidebar/horizontal scroll right | `scroll_right = 'o'` (general) |
| `Down` / `Up` | Highlight next/prev row | `scroll_down = 'Down'` (listing override; supports hold-repeat via ZMK NAV layer) |
| `gg` (vim-style) | Top of list | new — gives keyboard parity with browser scroll-to-top |
| `G` | Bottom of list | new |
| `Enter` | Open highlighted message/thread | meli default |
| `Backspace` | Go back (close tab in meli; browser back here) | `exit_entry = 'Backspace'` (listing) |
| `s` | Set seen (mark read without opening) | `set_seen = 's'` (listing) |
| `d` | Trash (POST `/api/trash`) | `commands` shortcut `d` (listing) — `tag add trash, tag remove inbox, flag set trash` |
| `D` | Un-trash (POST `/api/untrash`; restores `+inbox -trash`) | new (2026-05-06) — mirrors `A`'s relationship to `a`. Use from the trash view; harmless no-op elsewhere. |
| `a` | Archive (POST `/api/archive`) | new — fills the triage triad. Suggested in meli config comments: "Strongest candidate: `n` = archive". We use `a` because `n` is needed for sidebar nav. |
| `A` | Un-archive (POST `/api/unarchive`; `+inbox -archive`) | mirrors `D`/`d` for the trash pair |
| `K` | Kill-sender (POST `/api/mailcurator/blacklist` with `{msg_id}` or `{thread_id}`) | new (2026-05-06) — counterpart to message-view `K`. Server resolves the From-domain from the row's notmuch id, blacklists it in mailcurator, retro-trashes existing messages from that sender. Confirm dialog quotes the visible from-name. |
| `Ctrl+D` | Trash all messages matching the active filter (POST `/api/listing/trash-all`) | new (2026-05-06) — bulk-delete companion to `/`. Refuses (with toast) when no `?q=` filter is active so a misbinding can't accidentally trash a whole mailbox. Server-side guard also enforces non-empty `q`. Confirm dialog quotes the message count from the status-banner and the verbatim filter string. Use `D` from the trash view to restore individuals. |
| `S` | Sweep (run mailcurator policy matching this row) | new — see `data-curator-policies` row attr |
| `U` | Unsubscribe (RFC 8058 / mailto) | new — see `data-has-unsubscribe` row attr |
| `/` | Focus in-mailbox search input | meli's `:` command bar maps roughly here |
| `r` | Reply to highlighted (`/mail/compose?reply=<id>`) | `reply` (envelope-view); promoting to listing gives one-keystroke reply from the list |
| `R` | Reply-all (TODO: not yet implemented) | meli `reply_to_all` |
| `f` | Forward highlighted (`/mail/compose?fwd=<id>`) | meli `forward` |
| `c` | Compose new (`/mail/compose`) | meli `new_message` (default `m` clashes with mailforge convention — use `c` for compose) |
| `Ctrl+R` | Refresh (re-fetch mailbox) | `refresh = 'C-r'` (listing) — runs an XHR to a small `/api/refresh` endpoint that triggers `mail-sync` server-side, then reloads the page |
| `E` | Next mailbox (sidebar down) | `next_mailbox = 'E'` (listing) |
| `I` | Prev mailbox (sidebar up) | `prev_mailbox = 'I'` (listing) |
| `N` | Next account | `next_account = 'N'` (listing) |
| `O` | Prev account | `prev_account = 'O'` (listing) |
| `n` (lowercase, post-search) | Next search result | `next_search_result = 'M-n'` in meli (Meta to free `n`); we keep `n` for sidebar but also accept it as next-result when search is active |
| `N` (after search) | Previous search result | `previous_search_result = 'N'` |

### Multi-select (deferred)

meli has multi-select via `Space`/`v`. mailforge can defer this to a v2 — most
trash/archive operations are per-message and per-keystroke is fine. If
needed, add `Space` to toggle row selection, `Ctrl+A` to select all
visible, with bulk operations applying to the selection set.

## Message (`/mail/m/<id>`)

Mirrors meli's envelope-view + pager.

| Key | Action |
|---|---|
| `e` / `i` / `Down` / `Up` | Scroll body down/up (pager-style) |
| `Space` / `Shift+Space` | Page down / page up |
| `Backspace` | Back to listing (browser back) |
| `r` | Reply (`/mail/compose?reply=<id>`) |
| `R` | Reply-all |
| `f` | Forward (`/mail/compose?fwd=<id>`) |
| `d` | Trash and go back to listing |
| `a` | Archive and go back to listing |
| `J` | Next message in mailbox (or thread) |
| `K` | Previous message in mailbox (or thread) |
| `m` | Open in mailforge viewer (forces full-render of HTML body in fullscreen iframe — the original mailforge escalation path; useful for messages where the inline iframe is too cramped) |
| `v` | Open the HTML view via the existing `?view=full` URL **AND** add this sender's domain to the auto-HTML trust list (fire-and-forget POST `/api/html-trusted/add`). On future messages from the domain, MailForge auto-renders HTML inline IF the message's `Authentication-Results` (DMARC/SPF/DKIM) passes. |
| `V` | Untrust this sender's domain (POST `/api/html-trusted/remove` then reload). The message re-renders as plaintext and the trust chip in the header disappears. |
| `Ctrl+R` | Reload the message (re-fetch from notmuch) |

## Thread (`/mail/t/<thread-id>`)

Same as message bindings, plus:

| Key | Action |
|---|---|
| `Tab` | Cycle focus between messages in the thread |
| `Shift+Tab` | Cycle focus backwards |
| `o` | Open/expand the focused message (`<details>` toggle) |

## Compose (`/mail/compose`)

Mirrors meli's `[shortcuts.composing]` plus standard form ergonomics.

| Key | Action |
|---|---|
| `Tab` / `Shift+Tab` | Move between fields (To, Cc, Bcc, Subject, Body) |
| `Ctrl+Enter` | Send (POST `/api/send`) |
| `Ctrl+S` | Save draft (POST `/api/draft`) |
| `Ctrl+E` | Open body in Helix (see compose.rs Helix-escalation note) |
| `Escape` | Confirm-discard modal: discard / save draft / cancel |
| `Ctrl+Shift+Backspace` | Discard immediately (no confirm) — for muscle memory |

When focus is in a field, all bare letter keys (a, b, c, ...) type as
expected. Only the modified shortcuts and Tab fire.

## Search (`/mail/search`)

| Key | Action |
|---|---|
| (when input focused) `Enter` | Submit search |
| (when input focused) `Escape` | Blur input, return to base context |
| `e` / `i` / `Down` / `Up` (when results focused) | Highlight next/prev result |
| `Enter` (when result highlighted) | Open message/thread |
| `/` | Re-focus the search input (clear and type new query) |

## Things deferred / out of scope

- **Multi-select**: see Listing section. Defer to v2.
- **Drag-and-drop attachments**: future composer enhancement.
- **Vim-style command mode (`:`)**: meli has it; mailforge browser context
  doesn't translate cleanly. URL bar serves the equivalent role
  ("`/mail/cohs/sent`" is just typed in the address bar).
- **Sticky modifiers / leader keys**: William's ZMK firmware handles
  sticky-mod for Cmd/Ctrl/etc.; not a JS responsibility.
- **Tag-add modal** (à la meli's `tag add foo`): defer; for now `/api/tag`
  is used directly via JS `prompt()`. Bind to `t` in listing context.

## Implementation notes for the JS agent

- **No framework**. Pure `addEventListener("keydown", handler)`.
- **One file**: `static/js/keys.js`. ~250 lines target.
- **Module shape**:
  ```js
  const dispatch = {
    listing:  { e: navDown, i: navUp, /* ... */ },
    message:  { /* ... */ },
    // ...
  };
  document.addEventListener("keydown", (ev) => {
    const ctx = document.body.dataset.context;
    const key = formatKey(ev);  // e.g. "Ctrl+Enter"
    const handler = dispatch[ctx]?.[key] ?? dispatch._base?.[key];
    if (handler) {
      ev.preventDefault();
      handler(ev);
    }
  });
  ```
- **`formatKey`**: returns "Ctrl+Enter", "Shift+Tab", or just "e".
  Ignore Caps Lock state.
- **Optimistic UI updates**: on `d` in listing, fade the row to opacity
  0.4 immediately, send the XHR, on 200 remove the row from the DOM, on
  error restore opacity and show a toast.
- **Toast helper**: `showToast(msg, kind)` where `kind` ∈ `{success,
  error}`. Renders into `<div id=toast-container>` (added to layout in
  `templates.rs::page`).

## Test matrix

For each context above, the agent should manually verify:

1. Each binding fires the documented action.
2. Bare letters don't fire when focus is in a `<textarea>`.
3. `Ctrl+...` shortcuts fire even with focus in `<textarea>`.
4. `Escape` clears modal state in all contexts.
5. `Backspace` navigates back in listing/message/thread; deletes a char in
   composer fields.

Browser parity: test in Chrome (William's primary) and Safari (macOS
default). Skip Firefox unless mailforge ever needs to ship beyond William's
own use.
