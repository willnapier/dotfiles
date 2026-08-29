// Claude Conversation Exporter
// Extracts conversation and downloads as JSON compatible with continuum
//
// Ordering note: every strategy below issues ONE querySelectorAll with a
// comma-joined selector rather than one query per role. querySelectorAll
// returns nodes in document order, so a single combined query preserves the
// user/assistant interleaving for free. Querying each role separately and
// concatenating would emit all prompts followed by all responses — which the
// downstream converter cannot repair, because it re-derives roles by
// alternating position.

(function() {
  'use strict';

  const USER_SELECTORS = [
    '[data-testid="user-message"]',
    '.font-user-message'
  ];

  const ASSISTANT_SELECTORS = [
    '.font-claude-response',
    '.font-claude-message',
    '[data-testid="assistant-message"]'
  ];

  function createExportButton() {
    if (document.getElementById('claude-export-btn')) return;

    const btn = document.createElement('button');
    btn.id = 'claude-export-btn';
    btn.innerHTML = '⬇️ Export JSON';
    btn.addEventListener('click', exportConversation);
    document.body.appendChild(btn);
  }

  // Classify a matched node by testing it against the user selector list.
  // Anything that is not a user message is a response.
  function isUserNode(el) {
    return USER_SELECTORS.some(sel => el.matches(sel) || el.closest(sel) !== null);
  }

  function collect(selectors) {
    const messages = [];
    const nodes = document.querySelectorAll(selectors.join(', '));

    nodes.forEach(el => {
      // Skip a node that merely contains another match — the inner one is the
      // real message and would otherwise be captured twice, once nested.
      if (el.querySelector(selectors.join(', '))) return;

      const text = cleanText(el);
      if (!text) return;

      messages.push({
        role: isUserNode(el) ? 'Prompt' : 'Response',
        say: text
      });
    });

    return messages;
  }

  function extractConversation() {
    // Strategy 1: Claude's own testid/class markers, both roles in one pass.
    let messages = collect([...USER_SELECTORS, ...ASSISTANT_SELECTORS]);
    if (messages.length > 0) {
      console.log('Claude Exporter: Found', messages.length, 'messages via role markers');
      return deduplicateMessages(messages);
    }

    // Strategy 2: turn containers. Claude wraps each exchange in a render-count
    // container; classify by whether a user marker sits inside.
    console.log('Claude Exporter: Trying turn-container strategy');
    const turns = document.querySelectorAll('div[data-test-render-count]');
    turns.forEach(turn => {
      const text = cleanText(turn);
      if (!text) return;
      messages.push({
        role: USER_SELECTORS.some(sel => turn.querySelector(sel)) ? 'Prompt' : 'Response',
        say: text
      });
    });
    if (messages.length > 0) return deduplicateMessages(messages);

    // Strategy 3: generic prose blocks, alternating from the user.
    console.log('Claude Exporter: Trying generic prose strategy');
    let isUser = true;
    document.querySelectorAll('div.prose, [class*="message-content"]').forEach(el => {
      const text = cleanText(el);
      if (!text || text.length < 5) return;
      messages.push({ role: isUser ? 'Prompt' : 'Response', say: text });
      isUser = !isUser;
    });

    return deduplicateMessages(messages);
  }

  function cleanText(el) {
    const clone = el.cloneNode(true);

    // Remove non-content elements. `[class*="thinking"]` drops the collapsed
    // extended-thinking panel, whose innerText is a duplicate of the answer's
    // preamble and would otherwise be exported as part of the response.
    clone.querySelectorAll('svg, button, input, select, nav, header, footer, script, style, [aria-hidden="true"], [class*="icon"], [class*="button"], [class*="thinking"], .action-buttons')
      .forEach(e => e.remove());

    return clone.innerText?.trim() || '';
  }

  // Remove duplicate messages that can occur from nested selectors
  function deduplicateMessages(messages) {
    const seen = new Set();
    return messages.filter(msg => {
      const key = msg.role + ':' + msg.say.substring(0, 100);
      if (seen.has(key)) return false;
      seen.add(key);
      return true;
    });
  }

  function getProject() {
    try {
      // Claude Projects put the project name in a breadcrumb above the thread.
      const crumb = document.querySelector('[data-testid="project-name"], a[href^="/project/"]');
      const name = crumb?.innerText?.trim();
      if (name && name !== 'Claude') return name;
    } catch (e) {
      console.log('Claude Exporter: getProject() failed:', e);
    }
    return '';
  }

  function getTitle() {
    const titleEl = document.querySelector('[data-testid="chat-menu-trigger"], h1');
    let title = titleEl?.innerText?.trim() || document.title || 'Claude Conversation';
    // document.title is "<name> - Claude"; the header element is bare.
    title = title.replace(/\s*[-–—]\s*Claude\s*$/i, '').trim();
    return title || 'Claude Conversation';
  }

  function exportConversation() {
    const btn = document.getElementById('claude-export-btn');
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
        month: '2-digit', day: '2-digit', year: 'numeric'
      }) + ' ' + now.toLocaleTimeString('en-US', {
        hour: '2-digit', minute: '2-digit', second: '2-digit', hour12: false
      });

      const project = getProject();
      const conversation = {
        metadata: {
          user: { name: '', email: '' },
          dates: { created: dateStr, updated: dateStr, exported: dateStr },
          // detect_assistant() in chatgpt-to-continuum greps this string for a
          // vendor name, so "Claude" must appear here verbatim.
          powered_by: 'Claude Exporter (custom extension)'
        },
        messages: messages,
        title: title,
        project: project
      };

      const blob = new Blob([JSON.stringify(conversation, null, 2)], { type: 'application/json' });
      const url = URL.createObjectURL(blob);
      const a = document.createElement('a');
      a.href = url;
      // Filename prefix is load-bearing: ai-export-watcher matches
      // ^(ChatGPT|Grok|Gemini|Claude)-.*\.json$ before importing.
      a.download = `Claude-${title.replace(/[^a-zA-Z0-9 ]/g, '').substring(0, 50)}.json`;
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
      console.error('Claude Exporter error:', err);
      btn.innerHTML = '❌ Export failed';
      btn.classList.add('error');
      setTimeout(() => {
        btn.innerHTML = '⬇️ Export JSON';
        btn.classList.remove('error');
      }, 3000);
    }
  }

  function init() {
    if (document.readyState === 'loading') {
      document.addEventListener('DOMContentLoaded', createExportButton);
    } else {
      createExportButton();
    }

    const observer = new MutationObserver(() => {
      if (!document.getElementById('claude-export-btn')) {
        createExportButton();
      }
    });
    observer.observe(document.body, { childList: true, subtree: true });
  }

  init();
})();
