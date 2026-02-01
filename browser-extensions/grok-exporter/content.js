// Grok Conversation Exporter
// Extracts conversation and downloads as JSON compatible with continuum

(function() {
  'use strict';

  function createExportButton() {
    if (document.getElementById('grok-export-btn')) return;

    const btn = document.createElement('button');
    btn.id = 'grok-export-btn';
    btn.innerHTML = '⬇️ Export JSON';
    btn.addEventListener('click', exportConversation);
    document.body.appendChild(btn);
  }

  function extractConversation() {
    const messages = [];

    // Grok uses Tailwind CSS (migrated Jan 2025)
    // User messages: have bg-surface-l1 background class
    // Grok messages: have max-w-none class (full-width responses)
    // Message bubbles: .message-bubble class
    // Markdown responses: .response-content-markdown

    // Strategy 1: Find message bubbles and classify by styling
    const bubbles = document.querySelectorAll('.message-bubble');

    if (bubbles.length > 0) {
      console.log('Grok Exporter: Found', bubbles.length, 'message bubbles');

      bubbles.forEach(bubble => {
        const text = cleanText(bubble);
        if (!text) return;

        // User messages have bg-surface-l1 styling
        const isUser = bubble.closest('[class*="bg-surface-l1"]') !== null ||
                       bubble.matches('[class*="bg-surface-l1"]');

        messages.push({
          role: isUser ? 'Prompt' : 'Response',
          say: text
        });
      });
    }

    // Strategy 2: Look for markdown response containers + user inputs
    if (messages.length === 0) {
      console.log('Grok Exporter: Trying markdown container strategy');

      const responses = document.querySelectorAll('.response-content-markdown, [class*="max-w-none"]');
      const userInputs = document.querySelectorAll('[class*="bg-surface-l1"]');

      // Try to pair them up
      const allElements = [...document.querySelectorAll('[class*="bg-surface-l1"], .response-content-markdown, [class*="max-w-none"]')];

      allElements.forEach(el => {
        const text = cleanText(el);
        if (!text) return;

        const isUser = el.matches('[class*="bg-surface-l1"]') ||
                       el.closest('[class*="bg-surface-l1"]') !== null;

        messages.push({
          role: isUser ? 'Prompt' : 'Response',
          say: text
        });
      });
    }

    // Strategy 3: Generic turn-based extraction using dir="ltr" containers
    if (messages.length === 0) {
      console.log('Grok Exporter: Trying generic turn strategy');

      const turns = document.querySelectorAll('div[dir="ltr"]');
      let isUser = true;

      turns.forEach(turn => {
        const text = cleanText(turn);
        if (!text || text.length < 5) return;

        messages.push({
          role: isUser ? 'Prompt' : 'Response',
          say: text
        });
        isUser = !isUser;
      });
    }

    return deduplicateMessages(messages);
  }

  function cleanText(el) {
    const clone = el.cloneNode(true);

    // Remove non-content elements
    clone.querySelectorAll('svg, button, input, select, nav, header, footer, script, style, [aria-hidden="true"], [class*="icon"], [class*="button"], .action-buttons')
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

  function getTitle() {
    // Try page title, then conversation header elements
    const titleEl = document.querySelector('h1, [class*="conversation-title"], title');
    let title = titleEl?.innerText?.trim() || document.title || 'Grok Conversation';
    title = title.replace(/^Grok\s*[-–—]\s*/i, '').trim();
    return title || 'Grok Conversation';
  }

  function exportConversation() {
    const btn = document.getElementById('grok-export-btn');
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

      const conversation = {
        metadata: {
          user: { name: '', email: '' },
          dates: { created: dateStr, updated: dateStr, exported: dateStr },
          powered_by: 'Grok Exporter (custom extension)'
        },
        messages: messages,
        title: title
      };

      const blob = new Blob([JSON.stringify(conversation, null, 2)], { type: 'application/json' });
      const url = URL.createObjectURL(blob);
      const a = document.createElement('a');
      a.href = url;
      a.download = `Grok-${title.replace(/[^a-zA-Z0-9 ]/g, '').substring(0, 50)}.json`;
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
      console.error('Grok Exporter error:', err);
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
      if (!document.getElementById('grok-export-btn')) {
        createExportButton();
      }
    });
    observer.observe(document.body, { childList: true, subtree: true });
  }

  init();
})();
