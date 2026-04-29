% MELI-MINE(1) Personal meli quick reference | William Napier
% William Napier
% 2026-04-27

# NAME

meli-mine - personal meli reference for William's Colemak-DH neio config

# SYNOPSIS

Personal cheatsheet reflecting the **actual** keybindings, listing style, and account
layout configured in `~/.config/meli/config.toml`. Upstream `man meli` and
`man meli.conf` remain canonical for behaviour not customized here.

# LAYOUT

Two accounts share one notmuch database at `~/Mail`:

- **workspace** -- Gmail (will@willnapier.com), pulled by `lieer`/`mail-sync`
- **cohs** -- COHS Microsoft 365 (will.napier@changeofharleystreet.com), pulled by `mbsync`

Listing style is `Plain` with `relative_list_indices = true` -- one-line-per-message,
vim-style relative row numbers (selected row keeps its absolute index).

**Workspace inbox excludes cohs-tagged messages** (tagged via the post-new hook
on path under ~/Mail/cohs/) -- those appear only in the cohs account view.

**All-mail window is 30 days**. Older content reachable via `/` search.

# NAVIGATION

In the **listing, pager, and thread-view** the `[shortcuts.listing|pager|
thread-view]` blocks override `scroll_up`/`scroll_down` to `Up`/`Down` (arrows),
which shadows the `i`/`e` defined in `[shortcuts.general]`. Use the **ZMK NAV
layer** for vertical movement there (hold-repeat friendly). `n`/`o` taps still
fire as horizontal scroll because those contexts don't override
`scroll_left`/`scroll_right`, so the general bindings fall through.

In **modal dialogues and forms** (send confirmation, account picker, compose
field forms) `i`/`e` taps **do** work as up/down selectors — those widgets
don't override the general scroll bindings.

  Action                          Key
  ------------------------------- -----------------------------------
  Up / Down (listing/pager)       `Up` / `Down` (NAV layer)
  Up / Down (dialogues / forms)   `i` / `e` (tap) or `Up` / `Down`
  Left / Right (horizontal scroll) `n` / `o` (general scroll, falls through)

# AVAILABLE KEYS

Currently unbound and free for new shortcuts:

- **`n`** — taps only fire horizontal scroll (rarely useful in a one-line-per-
  message listing). Strong candidate for **archive** (`tag remove inbox`),
  filling the biggest gap in the triage triad alongside `d` (trash).
- **`o`** — same status as `n`. Candidates: toggle flagged (`tag add flagged`),
  mark unread (`tag add unread`), or save until a real need surfaces.
  Top / bottom of listing         `Home` / `End`
  Page up / page down             `PageUp` / `PageDown`
  Jump N rows                     digit then `Up`/`Down` (e.g. `5 Down`)
  Open selected                   `Enter`
  Back / exit current view        `Backspace` (NOT `Esc`, NOT `i`) — works in both listing and envelope-view
  Cancel command mode (`:` prompt) `Esc` — if you brush the colon key and a small prompt appears top-of-screen, this dismisses it
  Next / previous mailbox         `E` / `I`
  Next / previous account         `N` / `O`
  Jump to mailbox by sidebar #    `:go N` ↵
  Mark seen                       `s`
  Toggle help / shortcuts         `?` (contextual; differs per pane)

`Backspace` is the rebound `exit_entry` because meli's listing widgets consume `Esc`
internally before the shortcut dispatcher sees it.

# REFRESH

  Trigger                  Action
  ------------------------ ----------------------------------------------
  `Ctrl+R` in listing      Synchronous refresh + reindex for current account
  Automatic (workspace)    Every **5 min** via `com.user.mail-sync` launchd
  Automatic (cohs)         Every **15 min** via `com.williamnapier.mbsync-cohs`

`Ctrl+R` runs `refresh_command` from the account block:

- workspace: `/Users/williamnapier/.local/bin/mail-sync` (lieer pull + `notmuch new`)
- cohs:      `mbsync cohs && notmuch new`

There is no in-app last-refreshed indicator. Diagnostic logs:
`~/.local/share/mail-sync.log`, `~/Library/Logs/mbsync-cohs.log`.

**Tag staleness caveat.** meli's notmuch backend caches per-row tag state at
query time. `Ctrl+R` re-runs the query and surfaces new arrivals, but does NOT
refresh tag chips on already-listed rows. Tags applied to existing messages by
the post-new hook, mailcurator, or label-sync processes won't appear in a
long-lived meli session even after `Ctrl+R`. The `email` launcher mitigates
this by `pkill`-ing any existing meli before spawning a fresh one — the
convention is **one keypress = one fresh meli**, not "keep meli open all day".

The launcher also kickstarts both sync agents (`com.user.mail-sync` and
`com.williamnapier.mbsync-cohs` on Mac; `mail-sync.service` and
`mbsync-cohs.service` on Linux) at relaunch time, so a fresh meli sees mail
that's nearly current rather than up-to-5-min stale-from-last-tick. The pulls
run in parallel with meli init; notmuch indexing is available by the time
you'd next press `Ctrl+R`.

# READING

`Enter` on a row opens the envelope view. `Up`/`Down` (NAV layer) scroll the body —
`i`/`e` taps don't fire in the pager for the same reason as in the listing (see
NAVIGATION). They **do** work in the send-confirmation dialogue and other modal
forms. `Backspace` returns to the listing.

The flag column on each row uses single-character maildir flags:

  Char   Flag       Meaning
  ------ ---------- ---------------------------------------
  `S`    seen       you have read this message
  `○`    (unseen)   unread
  `R`    replied    you have replied
  `F`    flagged    starred / important
  `D`    draft      unfinished outgoing
  `T`    trashed    marked for deletion
  `P`    passed     forwarded

Multiple flags may appear together (e.g. `SR` = seen and replied).

# SEARCH

Press `/` from the listing to filter. Query syntax is **notmuch**, not free-text.
Useful patterns:

  Query                                     Matches
  ----------------------------------------- ----------------------------------
  `flags:unseen`                            unread messages
  `flags:replied`                           messages you replied to
  `from:vikki`                              from anyone matching "vikki"
  `subject:invoice`                         subject contains "invoice"
  `tag:cohs and tag:inbox`                  COHS inbox-tagged messages
  `date:7d..`                               last 7 days
  `date:2026-04-01..2026-04-30`             explicit date range
  `not tag:archive and tag:inbox`           true-inbox (not archived)

Combine with `and`, `or`, `not`. notmuch documentation: `man notmuch-search-terms`.

# SENDING

Per-account, no global override:

- **workspace** -- `msmtp --account=gmail` -- XOAUTH2 via `pizauth show gmail`
- **cohs** -- `graph-send` (standalone Rust tool, ~/.local/bin/graph-send)
  -- Microsoft Graph `/me/sendMail`. Token via `pizauth show cohs-graph`.
  COHS tenant blocks SMTP AUTH including XOAUTH2, so Graph is the only
  working send path. Source: ~/dotfiles/rust-projects/graph-send/.

# CONFIG SOURCE OF TRUTH

Edit `~/dotfiles/meli/config.toml`. The active config at `~/.config/meli/config.toml`
is a symlink managed by dotter. After editing:

```nushell
meli test-config ~/dotfiles/meli/config.toml
cd ~/dotfiles; git add meli/config.toml; git commit -m "meli: ..."; git push
```

Restart meli to pick up changes.

# UPDATING THIS PAGE

This page is generated from `~/dotfiles/meli/meli-mine.md` via:

```nushell
pandoc -s -t man ~/dotfiles/meli/meli-mine.md -o ~/dotfiles/meli/meli-mine.1
```

Then commit both files. Dotter symlinks the `.1` into `~/.local/share/man/man1/`.

# SEE ALSO

`man meli`, `man meli.conf`, `man meli-themes`, `man notmuch-search-terms`
