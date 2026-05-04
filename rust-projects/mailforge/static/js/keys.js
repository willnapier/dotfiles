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
          setTimeout(() => { row.remove(); paintCursor(); }, 200);
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
                    // Remove any visible rows from the same sender from
                    // the current page (others on later pages will be
                    // gone after the next listing reload).
                    document.querySelectorAll('tr.envelope-row').forEach(tr => {
                      const fromCell = tr.querySelector('.col-from .from-name');
                      if (fromCell && fromCell.textContent.includes(sender.split('@')[0])) {
                        tr.style.transition = "opacity 0.2s";
                        tr.style.opacity = "0";
                        setTimeout(() => { tr.remove(); paintCursor(); }, 200);
                      }
                    });
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
    // Templates stamp `data-msg-id` (→ dataset.msgId); legacy code paths
    // also accept `data-message-id` (→ dataset.messageId). Body comes
    // first because the page-level attribute wins over deeply nested ones.
    if (document.body.dataset.msgId) return document.body.dataset.msgId;
    if (document.body.dataset.messageId) return document.body.dataset.messageId;
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
  const msgNext = () => clickSel("[data-nav=next-message]");
  const msgPrev = () => clickSel("[data-nav=prev-message]");
  function msgHtmlView() {
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
    postForm("/api/draft", f)
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
        window.__helixPoll = setInterval(() => {
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
      n: nextMailbox, o: prevMailbox,
      N: nextAccount, O: prevAccount,
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
    document.addEventListener("keydown", handleKeydown, false);
    const ctx = document.body && document.body.dataset.context;
    if (ctx === "listing" || ctx === "search") paintCursor();

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

      // Resizable column widths. Restore from localStorage, then wire
      // mousedown handlers on each .col-resizer to drive live drag.
      // Persists pixel widths under "mailforge-col-widths" so the
      // values survive page reloads. To reset to defaults, the user
      // can clear the key from devtools or we could expose a
      // keyboard shortcut later.
      initResizableColumns();
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
