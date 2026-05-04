// MailForge — keys.js
// Vanilla JS keyboard navigation. Spec: docs/keybindings.md (authoritative).
// Layout: Colemak-DH neio = vim hjkl. event.key reports typed letter, so n/e/i/o
// map to left/down/up/right naturally.
//
// Dispatch: document.body.dataset.context selects the table
// (listing | message | thread | compose | search), with _base as fallback.
// Bare letter keys are suppressed when focus is in INPUT/TEXTAREA/SELECT.
// Modified keys (Ctrl/Alt) and Escape always fire.
//
// Embed snippet (for templates.rs::page):
//   <script defer src="/static/js/keys.js"></script>
//   <body data-context="listing">  ...required attribute...
//   <div id="toast-container"></div>  ...optional; self-heals if absent...
// Row contract: <tr class="envelope-row" data-id="<msg_id>"><a href="/mail/m/...">...
// Message contract: any element with data-message-id="<id>".
// Sidebar nav: anchors with data-nav="next-mailbox|prev-mailbox|next-account|
//   prev-account|next-message|prev-message" — J/K/N/O synthesise clicks on them.
// Compose: <form class="compose" action="/api/send"> with name=to/cc/bcc/subject/body.

(function () {
  "use strict";

  // ----- Helpers -----
  function formatKey(ev) {
    const p = [];
    if (ev.ctrlKey) p.push("Ctrl");
    if (ev.altKey) p.push("Alt");
    // Shift only encoded as a part for non-letter keys (letters carry case).
    if (ev.shiftKey && ev.key.length > 1) p.push("Shift");
    p.push(ev.key);
    return p.join("+");
  }

  function isTyping(el) {
    if (!el) return false;
    const t = el.tagName;
    return t === "INPUT" || t === "TEXTAREA" || t === "SELECT" || el.isContentEditable;
  }

  function showToast(msg, kind) {
    let host = document.getElementById("toast-container");
    if (!host) {
      host = document.createElement("div");
      host.id = "toast-container";
      host.style.cssText = "position:fixed;bottom:24px;right:24px;z-index:9999;display:flex;flex-direction:column;gap:8px;";
      document.body.appendChild(host);
    }
    // Read tokens from theme.css only — duplicate hex fallbacks
    // silently drift when theme.css moves, and the stylesheet is
    // always loaded by the time toasts appear (page() injects it
    // synchronously in the <head>).
    const border = kind === "error" ? "var(--error)" : kind === "success" ? "var(--success)" : "var(--rule)";
    const el = document.createElement("div");
    el.className = "toast toast-" + (kind || "info");
    el.textContent = msg;
    el.style.cssText = "padding:8px 12px;border-radius:4px;font-family:var(--font-mono);font-size:13px;"
      + "box-shadow:0 4px 12px rgba(0,0,0,0.4);background:var(--bg-elevated);color:var(--fg);border:1px solid " + border + ";";
    host.appendChild(el);
    setTimeout(() => { el.style.transition = "opacity 0.3s"; el.style.opacity = "0"; setTimeout(() => el.remove(), 300); }, 2400);
  }

  const postJSON = (url, body) => fetch(url, {
    method: "POST", credentials: "same-origin",
    headers: { "Content-Type": "application/json" },
    body: body ? JSON.stringify(body) : "{}",
  });
  // Send a form's fields as application/x-www-form-urlencoded.
  // `new FormData(f)` would produce multipart/form-data, but the
  // server's compose handler uses axum's Form<SendForm> extractor
  // which only accepts urlencoded — sending multipart returned 415.
  const postForm = (url, f) => {
    const params = new URLSearchParams();
    for (const [k, v] of new FormData(f).entries()) {
      params.append(k, typeof v === "string" ? v : "");
    }
    return fetch(url, {
      method: "POST",
      credentials: "same-origin",
      headers: { "Content-Type": "application/x-www-form-urlencoded" },
      body: params.toString(),
    });
  };

  // ----- Mailcurator sweep -----
  // "Sweep like this" — scopes to the policy of the row's message. Reads
  // `data-curator-policies` (set by templates.rs from the message's
  // `curator-<name>-seen` tags). Single-policy run is fast (no extractor
  // overhead) and intentional (you're looking at a row, you sweep its kind).
  //
  // Signature: sweepNow(rowEl?). Pass an explicit row (the per-row icon
  // case) or omit to use currentRow() (the keyboard-shortcut case).
  // Optional second arg `btn` is the icon button to disable while in
  // flight; not load-bearing.
  function sweepNow(rowEl, btn) {
    const row = rowEl || currentRow();
    if (!row) {
      showToast("No row selected", "error");
      return;
    }
    const policiesAttr = row.dataset.curatorPolicies || "";
    if (!policiesAttr) {
      showToast("No mailcurator policy matches this row", "info");
      return;
    }
    // Multi-policy case is rare; pick the first.
    const policy = policiesAttr.split(",")[0].trim();
    if (!confirm("Sweep all messages matched by policy '" + policy + "'?\n\n"
                 + "This ignores age thresholds. The extractor gate still applies "
                 + "(uncaptured data is never destroyed).")) {
      showToast("Cancelled", "info");
      return;
    }
    if (btn) btn.disabled = true;
    showToast("Sweeping " + policy + "…", "info");
    const url = "/api/mailcurator/sweep?only=" + encodeURIComponent(policy);
    fetch(url, { method: "POST", credentials: "same-origin" })
      .then(r => r.ok ? r.json() : Promise.reject(new Error("HTTP " + r.status)))
      .then(result => {
        if (!result.ok) throw new Error(result.error || "sweep failed");
        const msg = "Trashed " + (result.trashed || 0)
          + (result.archived ? ", archived " + result.archived : "");
        showToast(msg, "success");
        setTimeout(() => window.location.reload(), 600);
      })
      .catch(err => showToast("Sweep failed: " + err.message, "error"))
      .finally(() => { if (btn) btn.disabled = false; });
  }

  // ----- One-click unsubscribe -----
  // Reads `data-has-unsubscribe="true"` on the row to decide whether to
  // even ask. Then POSTs /api/unsubscribe/execute?id=<msg-id>. Server
  // handles the RFC 8058 one-click flow; if the message only has https
  // or mailto fallbacks, server returns `open_url` and we hand off to
  // the browser.
  // Parse an RFC 2369 `mailto:` List-Unsubscribe URL into to/subject/body.
  // Returns null if the URL doesn't start with mailto:. Multiple recipients
  // (comma-separated in the path) are preserved verbatim.
  //
  //   mailto:unsub@list.com?subject=unsubscribe
  //   → { to: "unsub@list.com", subject: "unsubscribe", body: "" }
  function parseMailto(url) {
    if (!url || !url.toLowerCase().startsWith("mailto:")) return null;
    try {
      // Strip "mailto:" then parse manually. URL() chokes on bare email
      // addresses without a scheme/host, so we split path? from query.
      const rest = url.slice(7); // after "mailto:"
      const qIdx = rest.indexOf("?");
      const path = qIdx >= 0 ? rest.slice(0, qIdx) : rest;
      const queryStr = qIdx >= 0 ? rest.slice(qIdx + 1) : "";
      const to = decodeURIComponent(path);
      const params = new URLSearchParams(queryStr);
      // Per RFC 6068 + 2369, the relevant query params are case-insensitive.
      // Walk all keys to handle different casings.
      let subject = "", body = "";
      for (const [k, v] of params.entries()) {
        const lk = k.toLowerCase();
        if (lk === "subject") subject = v;
        else if (lk === "body") body = v;
      }
      return { to, subject, body };
    } catch (e) {
      return null;
    }
  }

  function unsubscribeRow(rowEl, btn) {
    const row = rowEl || currentRow();
    if (!row) {
      showToast("No row selected", "error");
      return;
    }
    if (row.dataset.hasUnsubscribe !== "true") {
      showToast("No unsubscribe option for this row", "info");
      return;
    }
    const id = rowId(row);
    if (!id) {
      showToast("No message id on row", "error");
      return;
    }
    // Sender label for the confirm dialog: best-effort from the row's
    // From column. Strip the email half for readability.
    const fromCell = row.querySelector(".col-from .from-name") || row.querySelector(".col-from");
    const sender = (fromCell ? fromCell.textContent : "this sender").trim();
    if (!confirm("Unsubscribe from " + sender + "?\n\n"
                 + "If the sender supports RFC 8058 one-click, this is final. "
                 + "Otherwise the unsubscribe page opens in a new tab.")) {
      showToast("Cancelled", "info");
      return;
    }
    if (btn) btn.disabled = true;
    const url = "/api/unsubscribe/execute?id=" + encodeURIComponent(id);
    fetch(url, { method: "POST", credentials: "same-origin" })
      .then(r => r.json().then(j => ({ status: r.status, json: j })))
      .then(({ status, json }) => {
        if (json.ok) {
          showToast("Unsubscribed and trashed", "success");
          row.style.transition = "opacity 0.2s";
          row.style.opacity = "0";
          setTimeout(() => { row.remove(); decrementBannerCount(); paintCursor(); }, 200);
          // Post-unsub "scorched earth" follow-up: if the sender has
          // existing non-trashed messages, prompt to trash them all.
          // Brief delay so the success toast finishes painting before
          // the confirm dialog steals focus.
          const sender = json.sender_address;
          const count = json.sender_message_count || 0;
          if (sender && count > 0) {
            setTimeout(() => {
              const ok = window.confirm(
                "Unsubscribed.\n\n" + count + " existing message" + (count === 1 ? "" : "s") +
                " from " + sender + " on disk. Delete them all?"
              );
              if (!ok) return;
              showToast("Trashing " + count + " from " + sender + "…", "info");
              fetch("/api/unsubscribe/trash-from-sender?id=" + encodeURIComponent(id), {
                method: "POST",
                credentials: "same-origin",
              })
                .then(r => r.ok ? r.json() : Promise.reject(new Error("HTTP " + r.status)))
                .then(j => {
                  if (j.ok) {
                    showToast("Trashed " + j.count + " messages from " + (j.sender || sender), "success");
                    // Exact-id row removal. The server returns the bare
                    // message-ids it actually trashed; we walk the
                    // current page's envelope rows and remove only
                    // those matching by `data-msg-id`. Substring
                    // matching on the From column over-matches when
                    // multiple senders share a local-part (info@,
                    // noreply@, support@, notifications@…).
                    const trashedIds = new Set(Array.isArray(j.trashed_ids) ? j.trashed_ids : []);
                    if (trashedIds.size > 0) {
                      document.querySelectorAll('tr.envelope-row[data-msg-id]').forEach(tr => {
                        if (trashedIds.has(tr.dataset.msgId)) {
                          tr.style.transition = "opacity 0.2s";
                          tr.style.opacity = "0";
                          setTimeout(() => { tr.remove(); paintCursor(); }, 200);
                        }
                      });
                    }
                  } else {
                    showToast("Trash-all failed: " + (j.error || "unknown"), "error");
                  }
                })
                .catch(err => showToast("Trash-all failed: " + err.message, "error"));
            }, 700);
          }
          return;
        }
        if (json.open_url) {
          if (json.method === "mailto") {
            // Don't trigger the OS mailto handler (Mail.app on macOS).
            // Parse the mailto: URL and navigate to MailForge's compose
            // form pre-filled with the to/subject/body. The user can
            // review and Send via the existing compose pipeline, which
            // dispatches via the right account's pizauth+SMTP/Graph
            // backend. Carry `unsubscribe_for_id` so the server can tag
            // the ORIGINAL inbox row +unsubscribed +trash -inbox on a
            // successful Send, matching the one-click POST behaviour.
            const parsed = parseMailto(json.open_url);
            if (parsed && parsed.to) {
              const params = new URLSearchParams();
              params.set("to", parsed.to);
              if (parsed.subject) params.set("subject", parsed.subject);
              if (parsed.body) params.set("body", parsed.body);
              params.set("unsubscribe_for_id", id);
              window.location.href = "/mail/compose?" + params.toString();
              return;
            }
            // Fallback: parsing failed. Show the URL in a toast so the
            // user can copy it; don't auto-trigger Mail.app.
            showToast("Mailto unsubscribe — copy this URL: " + json.open_url, "info");
          } else {
            window.open(json.open_url, "_blank", "noopener,noreferrer");
            showToast("Opened unsubscribe page in new tab", "info");
          }
          return;
        }
        const errMsg = json.error || ("HTTP " + status);
        showToast("Unsubscribe failed: " + errMsg, "error");
      })
      .catch(err => showToast("Unsubscribe failed: " + err.message, "error"))
      .finally(() => { if (btn) btn.disabled = false; });
  }

  // ----- Listing cursor state -----
  let cursorIndex = 0;
  let lastKey = "", lastKeyAt = 0; // for vim-style "gg"

  const listingRows = () => Array.from(document.querySelectorAll("tr.envelope-row, .envelope-row, .row[data-id]"));
  const currentRow = () => listingRows()[cursorIndex] || null;

  function paintCursor() {
    const rows = listingRows();
    if (!rows.length) return;
    cursorIndex = Math.max(0, Math.min(rows.length - 1, cursorIndex));
    rows.forEach((r, i) => r.classList.toggle("kb-current", i === cursorIndex));
    const cur = rows[cursorIndex];
    if (cur && cur.scrollIntoView) cur.scrollIntoView({ block: "nearest" });
  }

  function moveCursor(delta) {
    const rows = listingRows();
    if (!rows.length) return;
    cursorIndex = Math.max(0, Math.min(rows.length - 1, cursorIndex + delta));
    paintCursor();
  }

  function pageMove(delta) {
    const rows = listingRows();
    if (!rows.length) return;
    const rh = rows[0].getBoundingClientRect().height || 24;
    moveCursor(delta * Math.max(1, Math.floor(window.innerHeight / rh) - 1));
  }

  function rowId(r) {
    if (!r) return null;
    // The template stamps `data-msg-id` on each <tr.envelope-row>; check
    // that first. The other two fallbacks are kept for future template
    // variants without forcing churn here.
    if (r.dataset.msgId) return r.dataset.msgId;
    if (r.dataset.id) return r.dataset.id;
    if (r.dataset.messageId) return r.dataset.messageId;
    const a = r.querySelector("a[data-msg-id], a[data-id]");
    if (!a) return null;
    return a.dataset.msgId || a.dataset.id || null;
  }

  function rowHref(r) {
    if (!r) return null;
    const a = r.querySelector("a[href]");
    return a ? a.getAttribute("href") : null;
  }

  // ----- Listing actions -----
  const navDown = () => moveCursor(1);
  const navUp = () => moveCursor(-1);
  const navPageDown = () => pageMove(1);
  const navPageUp = () => pageMove(-1);
  const navTop = () => { cursorIndex = 0; paintCursor(); };
  const navBottom = () => { cursorIndex = listingRows().length - 1; paintCursor(); };

  function openCurrent() {
    const href = rowHref(currentRow());
    if (href) window.location.href = href;
  }

  // Decrement the "<n> messages — page Y of Z" subtitle. Pure visual
  // patch; the next page render reads from notmuch and self-corrects.
  // Best-effort regex update — leaves subtitle alone if it doesn't
  // match the expected shape.
  function decrementBannerCount() {
    const sub = document.querySelector(".status-banner__subtitle");
    if (!sub) return;
    const txt = sub.textContent || "";
    const m = txt.match(/^(\d+)(\s+messages?)/);
    if (!m) return;
    const n = Math.max(0, parseInt(m[1], 10) - 1);
    sub.textContent = n + m[2] + txt.slice(m[0].length);
  }

  // Server expects `{ ids: [...] }` (bulk-friendly API). Single-row mutations
  // wrap the one id in a one-element array. Future multi-select can reuse
  // the same shape without API changes.
  //
  // Per-row in-flight flag (`data-in-flight="1"`) suppresses repeat key
  // presses on the same row while the network call is pending, so a
  // rapid "d d d d" doesn't fire four POSTs against the same id.
  function rowMutate(url, label) {
    const row = currentRow();
    if (!row) return;
    if (row.dataset.inFlight === "1") return;
    const id = rowId(row);
    if (!id) { showToast("No message id on row", "error"); return; }
    row.dataset.inFlight = "1";
    row.style.transition = "opacity 0.15s";
    row.style.opacity = "0.4";
    postJSON(url, { ids: [id] })
      .then(r => {
        if (!r.ok) throw new Error("HTTP " + r.status);
        row.remove();
        decrementBannerCount();
        showToast(label + "d", "success");
        paintCursor();
      })
      .catch(err => {
        row.style.opacity = "";
        delete row.dataset.inFlight;
        showToast(label + " failed: " + err.message, "error");
      });
  }
  const trashCurrent = () => rowMutate("/api/trash", "Trashe");
  const archiveCurrent = () => rowMutate("/api/archive", "Archive");
  const unarchiveCurrent = () => rowMutate("/api/unarchive", "Unarchive");

  function setSeenCurrent() {
    const row = currentRow();
    const id = rowId(row);
    if (!id) return;
    postJSON("/api/seen", { ids: [id] })
      .then(r => { if (!r.ok) throw new Error("HTTP " + r.status); row.classList.remove("unread"); showToast("Marked seen", "success"); })
      .catch(err => showToast("Seen failed: " + err.message, "error"));
  }

  function tagCurrent() {
    const id = rowId(currentRow());
    if (!id) return;
    const tag = window.prompt("Tag:");
    if (!tag) return;
    postJSON("/api/tag", { ids: [id], add: [tag] })
      .then(r => { if (!r.ok) throw new Error("HTTP " + r.status); showToast("Tagged " + tag, "success"); })
      .catch(err => showToast("Tag failed: " + err.message, "error"));
  }

  function focusSearchInput() {
    const el = document.querySelector(".search-form input[type=search], input[name=q], input[type=search]");
    if (el) { el.focus(); el.select && el.select(); }
  }

  function refresh() {
    // No /api/refresh route exists server-side; the previous POST
    // produced a 404 in the access log on every Ctrl+R. Just reload
    // the page — the listing handler will re-issue notmuch search/count
    // on render.
    window.location.reload();
  }

  function replyCurrent() {
    const id = rowId(currentRow());
    if (id) window.location.href = "/mail/compose?reply=" + encodeURIComponent(id);
  }
  function forwardCurrent() {
    const id = rowId(currentRow());
    if (id) window.location.href = "/mail/compose?fwd=" + encodeURIComponent(id);
  }
  const compose = () => { window.location.href = "/mail/compose"; };

  const clickSel = (sel) => { const el = document.querySelector(sel); if (el) el.click(); };
  // Sidebar walking: navigate by walking the actual data-account /
  // data-mailbox anchors instead of looking for data-nav attributes
  // (which the templates don't currently emit). Wraps at edges.
  function navMailbox(dir) {
    const links = Array.from(document.querySelectorAll("aside.sidebar a[data-mailbox]"));
    if (!links.length) return;
    let idx = links.findIndex((a) => a.classList.contains("active"));
    if (idx < 0) idx = 0;
    const next = (idx + dir + links.length) % links.length;
    const target = links[next].href;
    if (target) window.location.href = target;
  }
  function navAccount(dir) {
    const links = Array.from(document.querySelectorAll("aside.sidebar a[data-mailbox]"));
    if (!links.length) return;
    const accounts = [];
    let activeAccount = null;
    for (const a of links) {
      const acct = a.dataset.account;
      if (!acct) continue;
      if (!accounts.includes(acct)) accounts.push(acct);
      if (a.classList.contains("active")) activeAccount = acct;
    }
    if (accounts.length < 2 || !activeAccount) return;
    const cur = accounts.indexOf(activeAccount);
    const nextAcct = accounts[(cur + dir + accounts.length) % accounts.length];
    // Land on the first mailbox of the next account.
    const target = links.find((a) => a.dataset.account === nextAcct);
    if (target && target.href) window.location.href = target.href;
  }
  const nextMailbox = () => navMailbox(+1);
  const prevMailbox = () => navMailbox(-1);
  const nextAccount = () => navAccount(+1);
  const prevAccount = () => navAccount(-1);

  // ----- Message-view actions -----
  function scrollBody(delta) {
    const t = document.querySelector(".message-body") || document.scrollingElement;
    if (t === document.scrollingElement) window.scrollBy({ top: delta });
    else if (t) t.scrollTop += delta;
  }
  const msgScrollDown = () => scrollBody(40);
  const msgScrollUp = () => scrollBody(-40);
  const msgPageDown = () => scrollBody(window.innerHeight * 0.9);
  const msgPageUp = () => scrollBody(-window.innerHeight * 0.9);

  function currentMessageId() {
    // Templates stamp `data-msg-id` on the <article> for messages /
    // envelope <tr> for listings. The body element only carries
    // data-context — the previous body-first branches were dead.
    const el = document.querySelector("[data-msg-id], [data-message-id]");
    if (!el) return null;
    return el.dataset.msgId || el.dataset.messageId || null;
  }

  function msgGoto(qs) {
    const id = currentMessageId();
    if (id) window.location.href = "/mail/compose?" + qs + "=" + encodeURIComponent(id);
  }
  const msgReply = () => msgGoto("reply");
  const msgReplyAll = () => msgGoto("replyAll");
  const msgForward = () => msgGoto("fwd");

  function msgMutate(url, verb) {
    const id = currentMessageId();
    if (!id) return;
    // Server expects `{ ids: [...] }` — same shape as rowMutate. Wrap the
    // single id in a one-element array so axum's Json<IdsRequest> extractor
    // succeeds (otherwise: "Expected request with Content-Type: application/json").
    postJSON(url, { ids: [id] })
      .then(r => { if (!r.ok) throw new Error("HTTP " + r.status); history.back(); })
      .catch(err => showToast(verb + " failed: " + err.message, "error"));
  }
  const msgTrash = () => msgMutate("/api/trash", "Trash");
  const msgArchive = () => msgMutate("/api/archive", "Archive");
  const msgUnarchive = () => msgMutate("/api/unarchive", "Unarchive");
  // The message page emits exactly one [data-nav=...] per direction
  // (sibling_nav_links in message.rs). If a future thread or paginated
  // view emits multiple, querySelector's first-match-wins is the
  // contract — order in the DOM is "the next sibling to navigate to".
  const msgNext = () => clickSel("[data-nav=next-message]");
  const msgPrev = () => clickSel("[data-nav=prev-message]");
  // Extract the From-domain from the visible message header for trust-list
  // mutations. Walks the .meta-row entries for the one whose .meta-key is
  // "From", grabs the .meta-val text, then takes everything after the LAST
  // `@`. Returns "" when nothing recognisable is present so callers can
  // bail without surprising the server with an empty payload.
  function currentSenderDomain() {
    const rows = document.querySelectorAll(".message-header .meta-row");
    for (const row of rows) {
      const k = row.querySelector(".meta-key");
      if (!k || k.textContent.trim().toLowerCase() !== "from") continue;
      const v = row.querySelector(".meta-val");
      if (!v) return "";
      const text = v.textContent || "";
      // Strip any trailing `>` from `Name <a@b>` shapes; locate the rightmost @.
      const cleaned = text.replace(/[>\s]+$/, "").trim();
      const at = cleaned.lastIndexOf("@");
      if (at < 0) return "";
      let dom = cleaned.slice(at + 1).trim();
      // Strip a trailing `>` if it survived (e.g. "Name <a@b.com>").
      dom = dom.replace(/[>\s]+$/, "");
      return dom.toLowerCase();
    }
    return "";
  }

  function msgHtmlView() {
    // Side-effect: opt this domain into HTML auto-render for next time.
    // Fire-and-forget — don't block the navigation on the trust-add round-trip
    // (the navigation is the user-visible action; the server is fast enough
    // that the POST usually completes before the next page renders, but if
    // it doesn't, the trust-add lands the next page load down the line).
    const dom = currentSenderDomain();
    if (dom) {
      try {
        postJSON("/api/html-trusted/add", { domain: dom }).catch(() => {});
      } catch (_) { /* network errors get logged in DevTools */ }
    }
    // Prefer clicking the visible "HTML view" link — guarantees the same
    // navigation as a pointer click, which is the path the user has
    // already verified works. Falls back to URL synthesis only if the
    // link isn't on the page (e.g. action toolbar suppressed for some
    // reason).
    const link = document.querySelector('a[data-action="open-viewer"]');
    if (link) { link.click(); return; }
    const id = currentMessageId();
    if (id) window.location.href = "/mail/m/" + encodeURIComponent(id) + "?view=full";
  }

  // Capital-V — untrust the current sender's domain and reload so the
  // message re-renders as plaintext (the chip disappears too).
  function msgUntrustDomain() {
    const dom = currentSenderDomain();
    if (!dom) {
      showToast("No sender domain found on this message", "error");
      return;
    }
    postJSON("/api/html-trusted/remove", { domain: dom })
      .then(r => r.ok ? r.json() : Promise.reject(new Error("HTTP " + r.status)))
      .then(j => {
        if (!j.ok) throw new Error(j.error || "untrust failed");
        showToast("Untrusted " + dom + " — reloading…", "success");
        setTimeout(() => window.location.reload(), 300);
      })
      .catch(err => showToast("Untrust failed: " + err.message, "error"));
  }
  const msgReload = () => window.location.reload();

  // ----- Thread actions -----
  function threadCycle(delta) {
    const items = Array.from(document.querySelectorAll(".thread > details"));
    if (!items.length) return;
    let idx = items.findIndex(d => d === document.activeElement || d.contains(document.activeElement));
    idx = (idx + delta + items.length) % items.length;
    const target = items[idx];
    target.focus && target.focus();
    target.scrollIntoView({ block: "nearest" });
  }
  const threadNext = () => threadCycle(1);
  const threadPrev = () => threadCycle(-1);
  function threadToggle() {
    const d = document.activeElement && document.activeElement.closest("details");
    if (d) d.open = !d.open;
  }

  // ----- Compose actions -----
  // Tightened to id-only — the previous `form[action*='send']` clause
  // would also match a future "Send test email" form or anything else
  // posting to /api/send. The compose template stamps id="compose-form"
  // on the canonical form (compose.rs::compose_form_view).
  const composeForm = () => document.getElementById("compose-form");

  function composeSend() {
    const f = composeForm();
    if (!f) return;
    // Disable the Send button immediately so multiple clicks during the
    // (multi-second) SMTP+OAuth round-trip can't fire multiple sends.
    // Re-enable only on error — on success we navigate away anyway.
    const sendBtn = f.querySelector('button[type="submit"]');
    if (sendBtn) {
      if (sendBtn.dataset.sending === "true") return; // already in flight
      sendBtn.dataset.sending = "true";
      sendBtn.disabled = true;
    }
    showToast("Sending…", "info");
    postForm(f.getAttribute("action") || "/api/send", f)
      .then(r => {
        if (!r.ok) throw new Error("HTTP " + r.status);
        showToast("Sent", "success");
        setTimeout(() => { window.location.href = "/mail"; }, 600);
      })
      .catch(err => {
        showToast("Send failed: " + err.message, "error");
        if (sendBtn) {
          sendBtn.disabled = false;
          delete sendBtn.dataset.sending;
        }
      });
  }

  function composeSaveDraft() {
    const f = composeForm();
    if (!f) return;
    // Server is `Json<DraftBody>` — POSTing urlencoded returns 415.
    // Build the JSON object from the form fields by name. Optional
    // fields (cc/bcc/in_reply_to) coerce empty string → null so the
    // serde `Option<String>` extractor accepts them.
    const fd = new FormData(f);
    const opt = (k) => {
      const v = fd.get(k);
      return v && String(v).length > 0 ? String(v) : null;
    };
    const draftId = (f.dataset && f.dataset.draftId) || fd.get("draft_id") || "";
    const body = {
      id: String(draftId || ""),
      from_account: String(fd.get("from_account") || ""),
      to: String(fd.get("to") || ""),
      cc: opt("cc"),
      bcc: opt("bcc"),
      subject: String(fd.get("subject") || ""),
      body: String(fd.get("body") || ""),
      in_reply_to: opt("in_reply_to"),
    };
    postJSON("/api/draft", body)
      .then(r => { if (!r.ok) throw new Error("HTTP " + r.status); showToast("Draft saved", "success"); })
      .catch(err => showToast("Draft failed: " + err.message, "error"));
  }

  function composeEscalateHelix() {
    const f = composeForm();
    if (!f) return;
    const ta = f.querySelector("textarea[name=body]");
    const body = ta ? ta.value : "";
    postJSON("/api/escalate-helix", { body })
      .then(r => { if (!r.ok) throw new Error("HTTP " + r.status); return r.json(); })
      .then(j => {
        if (!j.ok) throw new Error(j.error || "escalation rejected");
        showToast("Helix opened — save in Helix to return; Ctrl+Shift+E to abort", "info");
        // Cap the poll at 600 ticks (1.5s × 600 = 15min). Past that
        // the user has almost certainly closed Helix without saving,
        // or the daemon stopped responding; either way, polling
        // forever is the wrong answer.
        let ticks = 0;
        const MAX_TICKS = 600;
        window.__helixPoll = setInterval(() => {
          ticks++;
          if (ticks > MAX_TICKS) {
            clearInterval(window.__helixPoll);
            window.__helixPoll = null;
            showToast("Helix escalation timeout — please refresh", "error");
            return;
          }
          fetch("/api/escalate-helix/status", { credentials: "same-origin" })
            .then(r => r.json())
            .then(j => {
              if (j && j.complete) {
                clearInterval(window.__helixPoll);
                window.__helixPoll = null;
                if (ta && typeof j.body === "string") ta.value = j.body;
                showToast("Helix returned", "success");
              }
            })
            .catch(() => {});
        }, 1500);
      })
      .catch(err => showToast("Helix escalation failed: " + err.message, "error"));
  }

  function composeAbortHelix() {
    if (window.__helixPoll) { clearInterval(window.__helixPoll); window.__helixPoll = null; }
    fetch("/api/escalate-helix/abort", { method: "POST", credentials: "same-origin" })
      .then(() => showToast("Helix escalation aborted", "info"))
      .catch(() => {});
  }

  function composeCancel() {
    const f = composeForm();
    let dirty = false;
    if (f) {
      f.querySelectorAll("input, textarea").forEach(el => {
        if ((el.value || "") !== (el.defaultValue || el.getAttribute("value") || "")) dirty = true;
      });
      // <select name="from_account"> uses defaultSelected on its
      // <option> children; el.value compares against the originally
      // selected option's value. Account changes carry no real cost
      // (you can switch back) but a silent loss-on-Esc is surprising.
      f.querySelectorAll("select").forEach(sel => {
        const orig = Array.from(sel.options).find(o => o.defaultSelected);
        const origVal = orig ? orig.value : (sel.options[0] && sel.options[0].value) || "";
        if ((sel.value || "") !== origVal) dirty = true;
      });
    }
    if (dirty && !window.confirm("Discard unsent message?")) return;
    // history.back() is a no-op when the compose page was opened
    // directly (bookmark, fresh tab, refresh). Detect that via the
    // referrer: if it's empty or cross-origin, history.back() won't
    // navigate within this app, so route explicitly to /mail.
    const sameOriginReferrer = document.referrer
      && document.referrer.startsWith(window.location.origin);
    if (sameOriginReferrer) {
      history.back();
    } else {
      window.location.href = "/mail";
    }
  }

  // ----- Modal/back -----
  // No overlay UI exists in MailForge; previously closeOverlays() was
  // wired to Escape in _base but only blurred the active element.
  // Bind Escape to back() directly so the meaning is honest. Compose
  // overrides Escape to composeCancel() in its dispatch table.
  //
  // On a message page, plain history.back() steps backward through
  // every message the user opened in this tab — confusing because
  // pressing Backspace once after opening multiple messages takes
  // them to the previous message, not back to the listing they
  // came from. Instead: record the listing URL in sessionStorage
  // on each listing/search page-load (see init()), and on
  // message-context Backspace jump straight to that URL. Falls
  // back to /mail (default inbox) if no listing has been visited
  // yet (e.g., direct-URL entry into a message), or to
  // history.back() for non-message contexts.
  const LISTING_KEY = "mailforge:lastListing";
  const back = () => {
    const ctx = document.body && document.body.dataset.context;
    if (ctx === "message" || ctx === "thread") {
      const last = sessionStorage.getItem(LISTING_KEY);
      if (last) { window.location.href = last; return; }
      window.location.href = "/mail";
      return;
    }
    history.back();
  };

  // ----- Dispatch tables -----
  // Letter keys are case-sensitive: ev.key encodes shift state for letters.
  const dispatch = {
    _base: {
      Escape: back,
      Backspace: back,
      "?": () => showToast("Help: see docs/keybindings.md", "info"),
    },

    listing: {
      e: navDown, i: navUp,
      // Three-tier vertical nav: e/i row (within mailbox), n/o mailbox
      // (within account), N/O account (across accounts). Lowercase
      // narrows scope, uppercase widens — mirrors meli's N/O convention.
      ArrowDown: navDown, ArrowUp: navUp,
      PageDown: navPageDown, PageUp: navPageUp,
      G: navBottom,
      g: () => {
        const now = Date.now();
        if (lastKey === "g" && now - lastKeyAt < 600) { navTop(); lastKey = ""; }
        else { lastKey = "g"; lastKeyAt = now; }
      },
      Enter: openCurrent,
      s: setSeenCurrent,
      d: trashCurrent, D: trashCurrent,
      a: archiveCurrent, A: unarchiveCurrent,
      "/": focusSearchInput,
      r: replyCurrent, f: forwardCurrent, c: compose,
      t: tagCurrent,
      "Ctrl+r": refresh, "Ctrl+R": refresh,
      // n is to the LEFT of o on the keyboard (Colemak-DH and QWERTY
      // both); n should mean "previous", o should mean "next". Matches
      // the message-context convention (n: msgPrev, o: msgNext).
      n: prevMailbox, o: nextMailbox,
      // Same N is left of O → N is "previous" in the spatial convention.
      N: prevAccount, O: nextAccount,
      // Per-row affordances: uppercase to keep the lowercase letter
      // namespace clear (lower-s is set-seen). U/S act on the cursor
      // row; same flow as clicking the per-row hover-reveal icons.
      U: () => unsubscribeRow(currentRow()),
      S: () => sweepNow(currentRow()),
    },

    message: {
      e: msgScrollDown, i: msgScrollUp,
      ArrowDown: msgScrollDown, ArrowUp: msgScrollUp,
      " ": msgPageDown, "Shift+ ": msgPageUp,
      r: msgReply, R: msgReplyAll, f: msgForward,
      d: msgTrash, a: msgArchive, A: msgUnarchive,
      n: msgPrev, o: msgNext,
      v: msgHtmlView,
      // Capital V — untrust this sender's domain (POST then reload).
      // Mirrors the lower-v "trust + open viewer" flow so the trust list
      // is fully keyboard-driven from the message view.
      V: msgUntrustDomain,
      "Ctrl+r": msgReload, "Ctrl+R": msgReload,
    },

    thread: {
      e: msgScrollDown, i: msgScrollUp,
      ArrowDown: msgScrollDown, ArrowUp: msgScrollUp,
      Tab: threadNext, "Shift+Tab": threadPrev,
      o: threadToggle,
      r: msgReply, R: msgReplyAll, f: msgForward,
      d: msgTrash, a: msgArchive, A: msgUnarchive,
    },

    compose: {
      "Ctrl+Enter": composeSend,
      "Ctrl+s": composeSaveDraft, "Ctrl+S": composeSaveDraft,
      "Ctrl+e": composeEscalateHelix, "Ctrl+E": composeEscalateHelix,
      "Ctrl+Shift+E": composeAbortHelix,
      "Ctrl+Shift+Backspace": back,
      Escape: composeCancel,
      // Tab/Shift+Tab use browser default; not in table.
    },

    search: {
      e: navDown, i: navUp,
      ArrowDown: navDown, ArrowUp: navUp,
      Enter: openCurrent,
      "/": focusSearchInput,
    },
  };

  // ----- Always-fire keys (modified shortcuts + Escape) -----
  function isAlwaysFire(key) {
    return key.indexOf("Ctrl+") === 0 || key.indexOf("Alt+") === 0 || key === "Escape";
  }

  // ----- Main handler -----
  function handleKeydown(ev) {
    // Always let Cmd/Meta-modified events through to the browser. Without
    // this, Cmd+R (reload), Cmd+T (new tab), Cmd+S (save), Cmd+F (find in
    // page), Cmd+Shift+R (hard reload) all collide with our `r`, `t`, `s`,
    // `f`, `R` bindings — formatKey() ignores metaKey, so Cmd+R formats
    // as plain "r" and msgReply fires, eating the reload. Native browser
    // shortcuts must always win on macOS.
    if (ev.metaKey) return;

    const ctx = (document.body && document.body.dataset.context) || "";
    const key = formatKey(ev);

    // Suppress bare letters in input/textarea — only modified shortcuts and Escape pass through.
    if (isTyping(document.activeElement) && !isAlwaysFire(key)) return;

    const ctxTable = dispatch[ctx];
    const handler = (ctxTable && ctxTable[key]) || dispatch._base[key];
    if (!handler) return;

    // Don't preventDefault for compose Tab/Shift+Tab — let browser handle field traversal.
    if (!(ctx === "compose" && (key === "Tab" || key === "Shift+Tab"))) {
      ev.preventDefault();
    }
    try {
      handler(ev);
    } catch (e) {
      // Defensive — never let a handler exception break the page.
      // eslint-disable-next-line no-console
      console.error("[mailforge keys]", e);
    }
  }

  // ----- Init -----
  function init() {
    // Reset the vim-"gg" prefix tracker on every page load so a stale
    // "g" pressed on the listing doesn't carry into the message view's
    // 600ms window.
    lastKey = "";
    lastKeyAt = 0;
    document.addEventListener("keydown", handleKeydown, false);
    const ctx = document.body && document.body.dataset.context;
    if (ctx === "listing" || ctx === "search") {
      paintCursor();
      // Remember this listing/search URL for message-page Backspace.
      // Each new listing visit overwrites — so the value always points
      // at the most recent listing the user actually navigated to.
      try { sessionStorage.setItem(LISTING_KEY, window.location.href); } catch (_) {}
    }

    // Focus rescue: when the user clicks a link inside the sandboxed
    // body iframe (typical case: an "unsubscribe" link with the
    // <base target="_blank"> from the email HTML, opening a new tab),
    // focus stays trapped in the iframe in the original tab. Keyboard
    // shortcuts like Backspace then no-op because keydown events from
    // the cross-origin sandboxed iframe don't bubble to the parent.
    // Two listeners restore focus to the outer document:
    //   1. window.focus — fires when the user switches back to this
    //      tab (from the unsubscribe-page tab, or from another window).
    //   2. document.click — fires when the user clicks on any chrome
    //      element outside the iframe (sidebar, helpbar, action toolbar).
    // Both blur the iframe if it's the active element, returning
    // keyboard focus to the parent document where keydown handlers fire.
    const rescueFocus = () => {
      const ae = document.activeElement;
      if (ae && ae.tagName === "IFRAME") {
        try { ae.blur(); } catch (_) {}
        if (document.body && document.body.tabIndex < 0) {
          document.body.setAttribute("tabindex", "-1");
        }
        try { document.body.focus(); } catch (_) {}
      }
    };
    window.addEventListener("focus", rescueFocus, false);
    document.addEventListener("click", (ev) => {
      // Only rescue if the click is on chrome (not on the iframe itself).
      if (ev.target && ev.target.tagName !== "IFRAME" && !ev.target.closest("iframe")) {
        rescueFocus();
      }
    }, true);

    // Forward keydown events from inside the body iframe back to the
    // parent's handler. The iframe sandbox includes allow-same-origin
    // (with body.html's strict CSP keeping it harmless) specifically
    // so we can attach this listener — otherwise contentDocument access
    // throws and clicks inside the iframe trap keys (Backspace, 1, 2,
    // e/i, etc. silently no-op). Re-dispatching synthesises a fresh
    // KeyboardEvent on the parent document; handleKeydown catches it
    // and routes via the dispatch table just like a native parent
    // keystroke.
    const wireIframeKeyForward = (iframe) => {
      const attach = () => {
        try {
          const doc = iframe.contentDocument;
          if (!doc) return;
          doc.addEventListener("keydown", (ev) => {
            // Don't hijack typing inside any input/textarea inside the
            // iframe (rare in mail bodies but possible — search forms in
            // newsletter HTML, for example).
            const t = ev.target;
            if (t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.isContentEditable)) {
              return;
            }
            // Synthesise an equivalent event on the parent document so
            // the existing keys.js dispatch table fires.
            const fwd = new KeyboardEvent("keydown", {
              key: ev.key,
              code: ev.code,
              ctrlKey: ev.ctrlKey,
              metaKey: ev.metaKey,
              altKey: ev.altKey,
              shiftKey: ev.shiftKey,
              bubbles: true,
              cancelable: true,
            });
            document.dispatchEvent(fwd);
          }, true);
        } catch (_) {
          // contentDocument access can throw if sandbox lacks
          // allow-same-origin. Fail closed — the parent's window.focus
          // / click rescue handlers above are the fallback.
        }
      };
      // The iframe may already be loaded by the time keys.js init runs
      // (loading="eager" + ready browser cache). Try immediately, then
      // also on the load event in case it isn't ready yet.
      attach();
      iframe.addEventListener("load", attach, false);
    };
    document.querySelectorAll("iframe.message-body__iframe").forEach(wireIframeKeyForward);

    // Intercept native form submit on the compose form so pressing Enter
    // (or clicking Send) goes through composeSend's XHR + post-success
    // redirect, instead of letting the browser navigate to /api/send and
    // render its JSON response.
    const cf = composeForm();
    if (cf) {
      cf.addEventListener("submit", (ev) => { ev.preventDefault(); composeSend(); }, false);
      const sd = document.getElementById("save-draft-now");
      if (sd) sd.addEventListener("click", (ev) => { ev.preventDefault(); composeSaveDraft(); }, false);
      const oh = document.getElementById("open-helix");
      if (oh) oh.addEventListener("click", (ev) => { ev.preventDefault(); composeEscalateHelix(); }, false);
    }

    // Per-row hover-reveal icons (sweep + unsubscribe). Event-delegated
    // off the listing tbody so we don't have to re-bind on each row;
    // listing rows can also be added/removed dynamically by row-mutation
    // helpers without losing handlers. The previous toolbar Sweep button
    // (data-action="sweep-now") was retired in favour of these per-row
    // controls — see listing.rs and the design note in mailforge.md.
    const ctxName = document.body && document.body.dataset.context;
    if (ctxName === "listing" || ctxName === "search") {
      document.addEventListener("click", (ev) => {
        const btn = ev.target.closest('.row-action[data-action]');
        if (!btn) return;
        const row = btn.closest('tr.envelope-row, .envelope-row');
        if (!row) return;
        ev.preventDefault();
        ev.stopPropagation();
        const action = btn.dataset.action;
        if (action === "sweep-row") sweepNow(row, btn);
        else if (action === "unsubscribe-row") unsubscribeRow(row, btn);
      }, false);

      // Whole-row click to open the message. Previously only the
      // subject column's <a> was clickable, but the hover highlight
      // covered the entire row — so clicks on From/Tags/Date columns
      // looked like they should work but didn't. Now: any click on
      // the row updates the keyboard cursor and navigates to the
      // message URL, matching the visual affordance. Skips clicks on
      // hover-reveal action buttons (handled above) and on real
      // anchors (browser navigates natively, but we still update the
      // cursor first so j/k after click resumes from the clicked row).
      document.addEventListener("click", (ev) => {
        if (ev.target.closest('.row-action')) return;
        const row = ev.target.closest('tr.envelope-row, .envelope-row');
        if (!row) return;
        const rows = listingRows();
        const idx = rows.indexOf(row);
        if (idx >= 0) {
          cursorIndex = idx;
          paintCursor();
        }
        if (ev.target.closest('a')) return; // browser handles navigation
        const link = row.querySelector('a[href*="/mail/m/"], a[href*="/mail/t/"]');
        if (link && link.href) window.location.href = link.href;
      }, false);

      // Resizable column widths. Restore from localStorage, then wire
      // mousedown handlers on each .col-resizer to drive live drag.
      // Persists pixel widths under "mailforge-col-widths" so the
      // values survive page reloads. To reset to defaults, the user
      // can clear the key from devtools or we could expose a
      // keyboard shortcut later.
      initResizableColumns();
    }

    // Trust-chip click handler: the auto-HTML chip in the message header
    // is a button with data-action="untrust-domain". Click → POST remove
    // → reload (so the message re-renders as plaintext and the chip
    // disappears). Lives in `message` and `thread` contexts (thread
    // headers may carry chips for individual messages once the trust
    // logic extends, though show_thread doesn't render them today).
    if (ctxName === "message" || ctxName === "thread") {
      document.addEventListener("click", (ev) => {
        const btn = ev.target.closest('button[data-action="untrust-domain"]');
        if (!btn) return;
        ev.preventDefault();
        ev.stopPropagation();
        const dom = btn.dataset.domain || "";
        if (!dom) {
          showToast("No domain on chip", "error");
          return;
        }
        btn.disabled = true;
        postJSON("/api/html-trusted/remove", { domain: dom })
          .then(r => r.ok ? r.json() : Promise.reject(new Error("HTTP " + r.status)))
          .then(j => {
            if (!j.ok) throw new Error(j.error || "untrust failed");
            showToast("Untrusted " + dom + " — reloading…", "success");
            setTimeout(() => window.location.reload(), 300);
          })
          .catch(err => {
            btn.disabled = false;
            showToast("Untrust failed: " + err.message, "error");
          });
      }, false);
    }

    // Sidebar keyboard shortcuts: number the first 9 sidebar anchors
    // 1-9 in render order, prepend visible kbd badges to each, and
    // bind digit keys 1-9 in _base so they work in any non-typing
    // context (listing, message, thread, search). Compose suppresses
    // digits while a field is focused (per isTyping check), so this
    // doesn't clash with typing numeric content into the form.
    initSidebarShortcuts();
  }

  function sidebarJumpHandler(n) {
    return () => {
      const a = document.querySelector(`aside.sidebar a[data-shortcut="${n}"]`);
      if (a && a.href) window.location.href = a.href;
    };
  }

  function initSidebarShortcuts() {
    const anchors = Array.from(document.querySelectorAll(
      "aside.sidebar a[data-mailbox], aside.sidebar a[data-action]"
    ));
    const numbered = anchors.slice(0, 9);
    numbered.forEach((a, i) => {
      const n = i + 1;
      a.dataset.shortcut = String(n);
      if (!a.querySelector(".sidebar-key")) {
        const kbd = document.createElement("kbd");
        kbd.className = "sidebar-key";
        kbd.textContent = String(n);
        a.insertBefore(kbd, a.firstChild);
      }
    });
    // Wire digit keys 1-9 to jumping. Done in _base so the bindings
    // work from any context (including message view); typing-suppression
    // (isTyping check in handleKeydown) prevents collision with numeric
    // input into form fields.
    for (let n = 1; n <= 9; n++) {
      dispatch._base[String(n)] = sidebarJumpHandler(n);
    }

    // Sidebar prefetch dropped 2026-05-04 — was firing 8 notmuch
    // subprocess searches per page load (500-800ms each, audit #14),
    // starving real clicks. William navigates by keyboard (j/k/n/o) so
    // hover-prefetch wouldn't help either. The trade-off: first click
    // to a cold mailbox is ~500-700ms instead of <100ms; second click
    // onwards uses notmuch's warm cache and is fast.
  }

  function initResizableColumns() {
    // Restore saved widths
    try {
      const raw = localStorage.getItem("mailforge-col-widths");
      if (raw) {
        const widths = JSON.parse(raw);
        for (const [col, val] of Object.entries(widths)) {
          if (typeof val === "string" && /^\d+(\.\d+)?(px|ch|em|rem)$/.test(val)) {
            document.documentElement.style.setProperty(`--col-${col}-width`, val);
          }
        }
      }
    } catch (e) { /* ignore corrupt localStorage */ }

    document.querySelectorAll(".col-resizer").forEach((handle) => {
      handle.addEventListener("mousedown", (ev) => {
        ev.preventDefault();
        ev.stopPropagation();
        const col = handle.dataset.col;
        if (!col) return;
        const th = handle.parentElement;
        const startX = ev.clientX;
        const startWidth = th.getBoundingClientRect().width;
        handle.classList.add("dragging");
        document.body.classList.add("col-resizing");

        const onMove = (mv) => {
          const delta = mv.clientX - startX;
          // Clamp to a sensible minimum so columns can't disappear.
          const newWidth = Math.max(40, Math.round(startWidth + delta));
          document.documentElement.style.setProperty(`--col-${col}-width`, newWidth + "px");
        };
        const onUp = () => {
          document.removeEventListener("mousemove", onMove);
          document.removeEventListener("mouseup", onUp);
          handle.classList.remove("dragging");
          document.body.classList.remove("col-resizing");
          // Persist all current widths
          try {
            const cs = getComputedStyle(document.documentElement);
            const widths = {};
            for (const c of ["from", "tags", "date", "actions"]) {
              const v = cs.getPropertyValue(`--col-${c}-width`).trim();
              if (v) widths[c] = v;
            }
            localStorage.setItem("mailforge-col-widths", JSON.stringify(widths));
          } catch (e) { /* localStorage might be full or disabled */ }
        };
        document.addEventListener("mousemove", onMove);
        document.addEventListener("mouseup", onUp);
      }, false);
    });
  }

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", init, { once: true });
  } else {
    init();
  }
})();
