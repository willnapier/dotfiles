// Gemini Conversation Exporter
// Extracts conversation and downloads as JSON compatible with continuum

(function() {
  'use strict';

  // Create export button
  function createExportButton() {
    if (document.getElementById('gemini-export-btn')) return;

    const btn = document.createElement('button');
    btn.id = 'gemini-export-btn';
    btn.innerHTML = '⬇️ Export JSON';
    btn.addEventListener('click', exportConversation);
    document.body.appendChild(btn);
  }

  // Drop any element whose ancestor is also in the matched set.
  // Gemini's DOM has nested classes (e.g. a query-text-container wraps a
  // query-text inner node), and broad [class*="..."] selectors match both.
  // Without this filter, every logical message gets captured twice — once
  // from the outer container and once from the inner text node, producing
  // the per-turn duplication seen in older exports.
  function filterToOutermost(nodeList) {
    const arr = Array.from(nodeList);
    return arr.filter(el => !arr.some(other => other !== el && other.contains(el)));
  }

  // Strip the UI label prefixes/suffixes that Gemini renders inline.
  // These markers are part of the rendered chat UI ("You said" above each
  // user message, "Gemini said" above each response, "Sources" footer
  // beneath responses with citations) and they get captured by innerText
  // even though they aren't part of the message content itself.
  function cleanMarkers(text) {
    if (!text) return '';
    let cleaned = text;
    // Leading "You said\n" or "You said\n\n"
    cleaned = cleaned.replace(/^You said\s*\n+/, '');
    // Leading "Gemini said\n" or "Gemini said\n\n"
    cleaned = cleaned.replace(/^Gemini said\s*\n+/, '');
    // Trailing "Sources" footer (with optional surrounding whitespace)
    cleaned = cleaned.replace(/\s*\n+Sources\s*$/, '');
    return cleaned.trim();
  }

  // Reject extractions that are just a model label (the "Gemini" header
  // string that appears as a sender label in some response containers).
  function isStrayLabel(text) {
    const t = (text || '').trim();
    return t === 'Gemini' || t === 'ChatGPT' || t === 'Claude' || t === 'Grok';
  }

  // Extract conversation from page
  function extractConversation() {
    const messages = [];

    // Gemini uses these actual class names (from inspection):
    // - query-text: user messages
    // - model-response-text / bard-text: model responses
    // - conversation-container: main chat area

    // Find user queries
    const userQueriesRaw = document.querySelectorAll('[class*="query-text"]');
    const userQueries = filterToOutermost(userQueriesRaw);

    // Find model responses - try multiple selectors
    const modelResponsesRaw = document.querySelectorAll('[class*="model-response-text"], [class*="bard-text"], .response-container-content');
    const modelResponses = filterToOutermost(modelResponsesRaw);

    console.log('Gemini Exporter: Found',
      userQueries.length, 'user queries (from', userQueriesRaw.length, 'raw matches),',
      modelResponses.length, 'model responses (from', modelResponsesRaw.length, 'raw matches)');

    // Interleave messages - Gemini alternates user/model
    const maxLen = Math.max(userQueries.length, modelResponses.length);

    for (let i = 0; i < maxLen; i++) {
      if (userQueries[i]) {
        const text = cleanMarkers(userQueries[i].innerText);
        if (text && text.length > 0 && !isStrayLabel(text)) {
          messages.push({ role: 'Prompt', say: text });
        }
      }
      if (modelResponses[i]) {
        const text = cleanMarkers(modelResponses[i].innerText);
        if (text && text.length > 0 && !isStrayLabel(text)) {
          messages.push({ role: 'Response', say: text });
        }
      }
    }

    // If that didn't work, try finding conversation turns by structure
    if (messages.length === 0) {
      console.log('Gemini Exporter: Trying turn-based extraction');

      // Look for conversation turns - each turn contains user + model message
      const turns = document.querySelectorAll('[class*="conversation-turn"], [class*="chat-turn"]');

      turns.forEach((turn) => {
        // User message usually comes first in the turn
        const userEl = turn.querySelector('[class*="query"], [class*="user"]');
        const modelEl = turn.querySelector('[class*="response"], [class*="model"], [class*="bard"]');

        if (userEl) {
          const text = userEl.innerText?.trim();
          if (text) messages.push({ role: 'Prompt', say: text });
        }
        if (modelEl) {
          const text = modelEl.innerText?.trim();
          if (text) messages.push({ role: 'Response', say: text });
        }
      });
    }

    return messages;
  }

  // Get project/Gem name if conversation is within a Gem
  function getProject() {
    try {
      // Try Gem name elements (Gemini organises custom instructions as "Gems")
      const gemEl = document.querySelector(
        '[class*="gem-name"], [data-gem-name], [class*="gem-title"]'
      );
      if (gemEl) {
        const name = gemEl.innerText?.trim();
        if (name) return name;
      }

      // Try breadcrumb or collection indicators
      const breadcrumbs = document.querySelectorAll('[class*="breadcrumb"] a, [class*="collection-name"]');
      for (const bc of breadcrumbs) {
        const name = bc.innerText?.trim();
        if (name && name !== 'Gemini' && name !== 'Home') return name;
      }
    } catch (e) {
      console.log('Gemini Exporter: getProject() failed:', e);
    }
    return '';
  }

  // Get conversation title
  function getTitle() {
    // Try various selectors for title
    const titleEl = document.querySelector(
      'h1, .conversation-title, [data-conversation-title], title'
    );

    let title = titleEl?.innerText?.trim() || document.title || 'Gemini Conversation';

    // Clean up title
    title = title.replace(/^Gemini\s*[-–—]\s*/i, '').replace(/\s*[-–—]\s*Google.*$/i, '').trim();

    return title || 'Gemini Conversation';
  }

  // Export conversation
  function exportConversation() {
    const btn = document.getElementById('gemini-export-btn');
    btn.innerHTML = '⏳ Exporting...';

    try {
      const messages = extractConversation();

      if (messages.length === 0) {
        btn.innerHTML = '❌ No messages found';
        btn.classList.add('error');
        setTimeout(() => {
          btn.innerHTML = '⬇️ Export JSON';
          btn.classList.remove('error');
        }, 3000);
        return;
      }

      const title = getTitle();
      const now = new Date();
      const dateStr = now.toLocaleDateString('en-US', {
        month: '2-digit',
        day: '2-digit',
        year: 'numeric'
      }) + ' ' + now.toLocaleTimeString('en-US', {
        hour: '2-digit',
        minute: '2-digit',
        second: '2-digit',
        hour12: false
      });

      const project = getProject();
      const conversation = {
        metadata: {
          user: { name: '', email: '' },
          dates: {
            created: dateStr,
            updated: dateStr,
            exported: dateStr
          },
          powered_by: 'Gemini Exporter (custom extension)'
        },
        messages: messages,
        title: title,
        project: project
      };

      // Create and download file
      const blob = new Blob([JSON.stringify(conversation, null, 2)], { type: 'application/json' });
      const url = URL.createObjectURL(blob);
      const a = document.createElement('a');
      a.href = url;
      a.download = `Gemini-${title.replace(/[^a-zA-Z0-9 ]/g, '').substring(0, 50)}.json`;
      document.body.appendChild(a);
      a.click();
      document.body.removeChild(a);
      URL.revokeObjectURL(url);

      btn.innerHTML = `✅ Exported ${messages.length} messages`;
      btn.classList.add('success');
      setTimeout(() => {
        btn.innerHTML = '⬇️ Export JSON';
        btn.classList.remove('success');
      }, 3000);

    } catch (err) {
      console.error('Gemini Exporter error:', err);
      btn.innerHTML = '❌ Export failed';
      btn.classList.add('error');
      setTimeout(() => {
        btn.innerHTML = '⬇️ Export JSON';
        btn.classList.remove('error');
      }, 3000);
    }
  }

  // Initialize
  function init() {
    // Wait for page to load
    if (document.readyState === 'loading') {
      document.addEventListener('DOMContentLoaded', createExportButton);
    } else {
      createExportButton();
    }

    // Re-add button if removed (SPA navigation)
    const observer = new MutationObserver(() => {
      if (!document.getElementById('gemini-export-btn')) {
        createExportButton();
      }
    });

    observer.observe(document.body, { childList: true, subtree: true });
  }

  init();
})();
