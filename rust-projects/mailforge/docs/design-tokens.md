# MailForge — Design Tokens & Conventions

Reference for the implementation agents that build the listing, message,
compose, and search views. Defines the palette, spacing, typography, and
component conventions used by `static/css/theme.css` and
`static/css/base.css`.

The aesthetic is **Solarized Dark** (Ethan Schoonover) — deliberately
matching William's Helix `solarized_dark_modal` theme and the
PracticeForge admin dashboard. Every implementation agent should consume
the tokens; do not hard-code colors, spacings, or font stacks.

---

## File layout

```
static/css/
  theme.css   — design tokens (variables only). Two themes via prefers-color-scheme.
  base.css    — reset, document defaults, typography, forms, layout primitives.
  mailforge.css (TODO) — component-specific styles added by impl agents.
```

`<head>` order: `theme.css` first, then `base.css`, then `mailforge.css`.

---

## Palette lineage

The palette is canonical Solarized — verified consistent across:

| Source | Path | Confirms |
|---|---|---|
| PracticeForge admin dashboard | `~/Code/practiceforge/practiceforge/src/admin_dashboard_assets/admin.html` (L14-23) | `--base03..base3`, all accents |
| PracticeForge login | `~/Code/practiceforge/practiceforge/src/admin_dashboard_assets/login.html` (L9-12) | dark surfaces + cyan/green/red |
| PracticeForge portal (public) | `~/Code/practiceforge/practiceforge/src/portal_assets/portal.html` (L9-11) | Solarized **light** equivalents |
| meli config | `~/dotfiles/meli/config.toml` (L214) | identical hex aliases |
| Ghostty terminal theme | `~/dotfiles/ghostty/themes/solarized-dark` | identical hex anchors |

`--rule: #0a4050` is a **custom dark teal** between `base03` and `base02`,
inherited from admin.html (L23). It gives panel separation without the
harsh contrast of `base02` against the body background.

---

## Token inventory

### Background layers

| Token | Dark value | Light value | Use |
|---|---|---|---|
| `--bg` | `#002b36` (base03) | `#fdf6e3` (base3) | Outermost background. Body, main content area. |
| `--bg-elevated` | `#073642` (base02) | `#eee8d5` (base2) | Sidebar, header, panels, table headers, form fields, button default. |
| `--bg-sunken` | `#001f27` | `#f7f0d8` | Inputs that should feel **below** the surface (textareas embedded in panels, code blocks). |
| `--bg-hover` | `rgba(255,255,255,0.03)` | `rgba(0,0,0,0.03)` | Row hover wash. Subtle. |

### Foreground

| Token | Dark | Light | Use |
|---|---|---|---|
| `--fg` | `#839496` (base0) | `#657b83` (base00) | Default body text. |
| `--fg-muted` | `#93a1a1` (base1) | `#586e75` (base01) | Slightly stronger — strong/em, headings, focused row text. |
| `--fg-faint` | `#586e75` (base01) | `#93a1a1` (base1) | Secondary text — placeholders, metadata, table headers. |

### Rules / borders

| Token | Value | Use |
|---|---|---|
| `--rule` | `#0a4050` (dark) / `base1` (light) | Default border for panels, inputs, table rows. |
| `--rule-strong` | `base01` | Heavier divider — section breaks. |

### Accent semantics

| Token | Maps to | Use |
|---|---|---|
| `--accent` | `--cyan` (`#2aa198`) | Primary action, focus ring, active tab, link, selected row outline. |
| `--accent-hover` | `#35b5ab` | Hover variant on solid-cyan buttons (lifted). |
| `--success` | `--green` (`#859900`) | Send-success toasts, "sent" tag, accept buttons. |
| `--warning` | `--yellow` (`#b58900`) | Drafts pending, "today" markers. |
| `--error` | `--red` (`#dc322f`) | Trash buttons, send failures, error banners. |
| `--info` | `--blue` (`#268bd2`) | Informational chips, From: headers (matches meli theme). |
| `--unread` | `--base2` (`#eee8d5`) | Bold-bright text in unread envelope rows (matches meli `mail.listing.compact.even_unseen`). |
| `--selected` | `rgba(42,161,152,0.18)` | Background fill for selected rows (e.g. keyboard cursor row). |

### Spacing scale (4px base)

| Token | Value | Typical use |
|---|---|---|
| `--sp-1` | `4px` | Tight gutters, inline padding. |
| `--sp-2` | `8px` | Form field padding, button padding-y. |
| `--sp-3` | `12px` | Panel padding-y, table cell padding, default flex gap. |
| `--sp-4` | `16px` | Panel padding-x, sidebar padding. |
| `--sp-5` | `20px` | Section gaps. |
| `--sp-6` | `24px` | Larger section gaps. |
| `--sp-7` | `32px` | Modal padding. |
| `--sp-8` | `48px` | Page-level vertical rhythm. |

### Type scale

| Token | px | Use |
|---|---|---|
| `--text-xs` | 11 | Micro-labels (key hints, tag chips, percent badges). |
| `--text-sm` | 13 | Secondary text — metadata, captions, button text. |
| `--text-base` | 15 | Body, table cells, form inputs. |
| `--text-lg` | 17 | h2, panel titles. |
| `--text-xl` | 20 | h1, page titles. |
| `--text-2xl` | 24 | Stat numbers, hero counters. |

### Font stacks

- `--font-mono` — primary. `'SF Mono', 'Cascadia Code', 'JetBrains Mono', 'Fira Code', ui-monospace, …`. Default body font. Matches admin.html (L26) and the editor aesthetic. **Use this for everything by default.**
- `--font-ui` — fallback for cases where a sans-serif is more appropriate (long-form prose in a message body). Reserve for the rendered email content area, not the chrome.

### Border radii

| Token | Value | Use |
|---|---|---|
| `--radius-sm` | 3px | Chips, badges, kbd, scrollbar thumb. |
| `--radius-md` | 4px | Inputs, buttons, panels (default). |
| `--radius-lg` | 6px | Modals, large cards. |

### Shadows

| Token | Value | Use |
|---|---|---|
| `--shadow-sm` | `0 2px 8px rgba(0,0,0,0.4)` | Inline pop-overs, undo bars. |
| `--shadow-md` | `0 4px 12px rgba(0,0,0,0.4)` | Modals, settings panels, toasts. |

### Focus

`--focus-ring` is a 2px cyan box-shadow at 45% alpha. Used on form fields
in `:focus` and `:focus-visible`. Buttons and links use the standard
`outline: 2px solid var(--accent); outline-offset: 2px` from `:focus-visible`.

The keyboard outline must be **unmissable** — William keyboard-navigates
everything. Implementation agents should never set `outline: none` on
interactive elements without providing an equivalent visual focus cue.

---

## Component conventions

### Sidebar (account / mailbox tree)

```html
<aside class="sidebar">
  <h2>personal</h2>
  <ul>
    <li><a href="/mail/personal/inbox" class="active">Inbox <span class="count">12</span></a></li>
    <li><a href="/mail/personal/sent">Sent</a></li>
  </ul>
</aside>
```

- `.sidebar` background = `--bg-elevated`, right border = `--rule`.
- Account heading is `<h2>` (uses `--fg-muted`).
- Mailbox rows have hover wash (`--bg-hover`) and `.active` gets a left
  border in `--accent` plus `color: var(--accent)`.

### Listing (envelope rows)

```html
<main class="main">
  <table class="listing">
    <thead>
      <tr><th>Tags</th><th>From</th><th>Subject</th><th>Date</th></tr>
    </thead>
    <tbody>
      <tr class="unread"><td>...</td>...</tr>
      <tr class="selected"><td>...</td>...</tr>
    </tbody>
  </table>
</main>
```

- Default row text = `--fg`.
- `.unread` rows: bold + `color: var(--unread)`.
- `.selected` (keyboard cursor): `background: var(--selected); outline: 1px solid var(--accent); outline-offset: -1px;` (mirrors admin.html L61).
- Hover row: `background: var(--bg-hover)`.

### Tag chip

```html
<span class="tag-chip">billing</span>
<span class="tag-chip success">sent</span>
<span class="tag-chip error">overdue</span>
```

- Default chip: `--bg-elevated` background, `--fg-faint` text, `--radius-sm`, padding `0 var(--sp-1)`, `font-size: var(--text-xs)`.
- Variants tint via accent semantics (15% alpha background, full-strength foreground) — mirrors admin.html badge family (L65-75).

### Helpbar (footer key hints)

```html
<footer class="helpbar">
  <kbd>i</kbd>/<kbd>e</kbd> nav
  <kbd>Enter</kbd> open
  <kbd>r</kbd> reply
  <kbd>/</kbd> search
</footer>
```

- Background `--bg-elevated`, top border `--rule`, padding `var(--sp-1) var(--sp-4)`.
- `<kbd>` already styled in base.css.
- Font size `--text-sm`, color `--fg-faint`.

### Compose form

```html
<form class="compose stack" method="POST" action="/api/send">
  <label>To <input type="email" name="to"></label>
  <label>Subject <input type="text" name="subject"></label>
  <label>Body <textarea name="body" rows="20"></textarea></label>
  <div class="cluster">
    <button type="submit" class="primary">Send</button>
    <button type="button">Save draft</button>
  </div>
</form>
```

- Use `.stack` for vertical rhythm between fields.
- Use `.cluster` for the action row.
- Textarea inherits mono font from base.css.

### Modal / overlay

```html
<div class="overlay">
  <div class="panel modal">
    <h2>Confirm send</h2>
    <p>...</p>
    <div class="cluster">
      <button>Cancel</button>
      <button class="primary">Send</button>
    </div>
  </div>
</div>
```

- Overlay: `position: fixed; inset: 0; background: rgba(0,0,0,0.7); display: flex; align-items: center; justify-content: center;`.
- `.modal`: `--shadow-md`, `--radius-lg`, max-width 420px.
- Implementation agents add `.overlay` and `.modal` modifiers in `mailforge.css`.

---

## When to use which token (quick reference)

- **Outermost surface?** `--bg`.
- **Card or sidebar floating on top?** `--bg-elevated`.
- **Input or code block sunk into a card?** `--bg-sunken`.
- **Hairline separator?** `--rule`.
- **Heavier section break?** `--rule-strong`.
- **Primary action, link, focus, selection?** `--accent`.
- **Bold unread text?** `--unread`.
- **Vertical rhythm between siblings?** Wrap in `.stack` (or use `--sp-3` directly).
- **Horizontal row of buttons?** Wrap in `.cluster`.
- **Modal shadow?** `--shadow-md`. **Inline tooltip shadow?** `--shadow-sm`.

---

## What this foundation does NOT define

These are intentionally left to the implementation agents who know the HTML structure:

- `.listing`, `.envelope-row`, `.envelope-row.unread` (listing component)
- `.compose`, `.draft-saved-indicator` (compose component)
- `.message-headers`, `.message-body`, `.thread > details` (message/thread component)
- `.search-form`, `.search-results` (search component)
- `.paginator`, `.paginator__current`
- `.tag-chip` modifiers beyond the base (per-tag colors if any)
- Iframe sandbox styling for the HTML body viewer
- Any keyboard-cursor styling beyond `.selected`

Add those in `mailforge.css`. They should compose the primitives here, not
redefine the palette or scale.

---

## Light mode behavior

Default is dark. The `@media (prefers-color-scheme: light)` block in
`theme.css` swaps surface and foreground tokens to Solarized light. The
accent palette stays cyan in both modes (works on both bases).

If the architect later decides this is dark-only (per the
`templates.rs` comment in mod scaffolding), removing the
`@media (prefers-color-scheme: light)` block in `theme.css` is a single
contiguous delete — no other file changes.

---

## Lineage notes for surprising decisions

- **Mono everywhere** — admin.html uses `monospace` for the entire body (L26). The legacy `style.css` (Iced/non-Helix dashboard) uses sans-serif, but the brief explicitly says "match the Helix theme more than meli's theme when there's a conflict" and admin.html *is* the Helix-aligned dashboard. Implementation agents should let prose-heavy content (rendered email bodies) opt into `--font-ui` if needed.
- **`--bg-sunken: #001f27`** is darker than `base03` — there's no canonical Solarized "below background" value, so I derived one. Used for inputs nested inside `--bg-elevated` panels so they read as receded.
- **`--rule: #0a4050`** comes from admin.html's `--border` (L23). It's a custom dark teal, not a Solarized canonical. Tighter contrast than `base02`-vs-`base03` would give.
- **Default button is outlined, not filled** (admin.html convention, L255). Only `.primary` (and `.danger` on hover) get tinted backgrounds. This keeps the chrome feeling like a TUI.
- **Two-theme support** despite architect's "dark-only" comment — the brief is explicit and more recent. Easy to remove if William prefers the architect's call.
