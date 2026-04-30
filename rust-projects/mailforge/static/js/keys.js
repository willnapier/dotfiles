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
    const border = kind === "error" ? "var(--error,#dc322f)" : kind === "success" ? "var(--success,#859900)" : "var(--rule,#0a4050)";
    const el = document.createElement("div");
    el.className = "toast toast-" + (kind || "info");
    el.textContent = msg;
    el.style.cssText = "padding:8px 12px;border-radius:4px;font-family:var(--font-mono,monospace);font-size:13px;"
      + "box-shadow:0 4px 12px rgba(0,0,0,0.4);background:var(--bg-elevated,#073642);color:var(--fg,#839496);border:1px solid " + border + ";";
    host.appendChild(el);
    setTimeout(() => { el.style.transition = "opacity 0.3s"; el.style.opacity = "0"; setTimeout(() => el.remove(), 300); }, 2400);
  }

  const postJSON = (url, body) => fetch(url, {
    method: "POST", credentials: "same-origin",
    headers: { "Content-Type": "application/json" },
    body: body ? JSON.stringify(body) : "{}",
  });
  const postForm = (url, f) => fetch(url, { method: "POST", credentials: "same-origin", body: new FormData(f) });

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

  // Server expects `{ ids: [...] }` (bulk-friendly API). Single-row mutations
  // wrap the one id in a one-element array. Future multi-select can reuse
  // the same shape without API changes.
  function rowMutate(url, label) {
    const row = currentRow();
    if (!row) return;
    const id = rowId(row);
    if (!id) { showToast("No message id on row", "error"); return; }
    row.style.transition = "opacity 0.15s";
    row.style.opacity = "0.4";
    postJSON(url, { ids: [id] })
      .then(r => { if (!r.ok) throw new Error("HTTP " + r.status); row.remove(); showToast(label + "d", "success"); paintCursor(); })
      .catch(err => { row.style.opacity = ""; showToast(label + " failed: " + err.message, "error"); });
  }
  const trashCurrent = () => rowMutate("/api/trash", "Trashe");
  const archiveCurrent = () => rowMutate("/api/archive", "Archive");

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
    postJSON("/api/refresh", {}).finally(() => window.location.reload());
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
    if (document.body.dataset.messageId) return document.body.dataset.messageId;
    const el = document.querySelector("[data-message-id]");
    return el ? el.dataset.messageId : null;
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
    postJSON(url, { id })
      .then(r => { if (!r.ok) throw new Error("HTTP " + r.status); history.back(); })
      .catch(err => showToast(verb + " failed: " + err.message, "error"));
  }
  const msgTrash = () => msgMutate("/api/trash", "Trash");
  const msgArchive = () => msgMutate("/api/archive", "Archive");
  const msgNext = () => clickSel("[data-nav=next-message]");
  const msgPrev = () => clickSel("[data-nav=prev-message]");
  function msgFullscreen() {
    const id = currentMessageId();
    if (id) window.location.href = "/mail/m/" + encodeURIComponent(id) + "?view=full";
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
  const composeForm = () => document.querySelector("form.compose, form#compose, form[action*='send']");

  function composeSend() {
    const f = composeForm();
    if (!f) return;
    postForm(f.getAttribute("action") || "/api/send", f)
      .then(r => { if (!r.ok) throw new Error("HTTP " + r.status); showToast("Sent", "success"); setTimeout(() => { window.location.href = "/mail"; }, 600); })
      .catch(err => showToast("Send failed: " + err.message, "error"));
  }

  function composeSaveDraft() {
    const f = composeForm();
    if (!f) return;
    postForm("/api/draft", f)
      .then(r => { if (!r.ok) throw new Error("HTTP " + r.status); showToast("Draft saved", "success"); })
      .catch(err => showToast("Draft failed: " + err.message, "error"));
  }

  function composeEscalateHelix() {
    const f = composeForm();
    if (!f) return;
    postForm("/api/escalate-helix", f)
      .then(r => { if (!r.ok) throw new Error("HTTP " + r.status); })
      .then(() => {
        showToast("Helix opened — Ctrl+Shift+E to abort", "info");
        window.__helixPoll = setInterval(() => {
          fetch("/api/escalate-helix/status", { credentials: "same-origin" })
            .then(r => r.json())
            .then(j => {
              if (j && j.complete) {
                clearInterval(window.__helixPoll);
                window.__helixPoll = null;
                const ta = document.querySelector("form.compose textarea[name=body], form#compose textarea[name=body]");
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
    }
    if (dirty && !window.confirm("Discard unsent message?")) return;
    history.back();
  }

  // ----- Modal/back -----
  function closeOverlays() {
    document.querySelectorAll(".overlay.open").forEach(o => o.classList.remove("open"));
    if (document.activeElement && document.activeElement !== document.body && document.activeElement.blur) {
      document.activeElement.blur();
    }
  }
  const back = () => history.back();

  // ----- Dispatch tables -----
  // Letter keys are case-sensitive: ev.key encodes shift state for letters.
  const dispatch = {
    _base: {
      Escape: closeOverlays,
      Backspace: back,
      "?": () => showToast("Help: see docs/keybindings.md", "info"),
    },

    listing: {
      e: navDown, i: navUp,
      // n/o = sidebar horizontal scroll in meli; in browser there's no equivalent, so they're
      // unbound here. Account nav uses uppercase N/O (per meli config: next_account='N', prev_account='O').
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
      a: archiveCurrent,
      "/": focusSearchInput,
      r: replyCurrent, f: forwardCurrent, c: compose,
      t: tagCurrent,
      "Ctrl+r": refresh, "Ctrl+R": refresh,
      E: nextMailbox, I: prevMailbox,
      N: nextAccount, O: prevAccount,
    },

    message: {
      e: msgScrollDown, i: msgScrollUp,
      ArrowDown: msgScrollDown, ArrowUp: msgScrollUp,
      " ": msgPageDown, "Shift+ ": msgPageUp,
      r: msgReply, R: msgReplyAll, f: msgForward,
      d: msgTrash, a: msgArchive,
      J: msgNext, K: msgPrev,
      m: msgFullscreen,
      "Ctrl+r": msgReload, "Ctrl+R": msgReload,
    },

    thread: {
      e: msgScrollDown, i: msgScrollUp,
      ArrowDown: msgScrollDown, ArrowUp: msgScrollUp,
      Tab: threadNext, "Shift+Tab": threadPrev,
      o: threadToggle,
      r: msgReply, R: msgReplyAll, f: msgForward,
      d: msgTrash, a: msgArchive,
      J: msgNext, K: msgPrev,
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
    document.addEventListener("keydown", handleKeydown, false);
    const ctx = document.body && document.body.dataset.context;
    if (ctx === "listing" || ctx === "search") paintCursor();
  }

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", init, { once: true });
  } else {
    init();
  }
})();
