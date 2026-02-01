// ChatGPT Conversation Exporter
// Extracts conversation and downloads as JSON compatible with continuum

(function() {
  'use strict';

  function createExportButton() {
    if (document.getElementById('chatgpt-export-btn')) return;

    const btn = document.createElement('button');
    btn.id = 'chatgpt-export-btn';
    btn.innerHTML = '⬇️ Export JSON';
    btn.addEventListener('click', exportConversation);
    document.body.appendChild(btn);
  }

  function extractConversation() {
    const messages = [];

    // Strategy 1: data-message-author-role attribute (most reliable when present)
    const authorRoleEls = document.querySelectorAll('[data-message-author-role]');

    if (authorRoleEls.length > 0) {
      console.log('ChatGPT Exporter: Found', authorRoleEls.length, 'messages via data-message-author-role');

      authorRoleEls.forEach(el => {
        const role = el.getAttribute('data-message-author-role');
        if (role === 'system') return; // Skip system messages

        const text = cleanText(el);
        if (!text) return;

        messages.push({
          role: role === 'user' ? 'Prompt' : 'Response',
          say: text
        });
      });
    }

    // Strategy 2: Look for message containers with user/assistant class patterns
    if (messages.length === 0) {
      console.log('ChatGPT Exporter: Trying class-based strategy');

      // ChatGPT groups messages in turn containers
      const turns = document.querySelectorAll('[data-testid^="conversation-turn"]');

      if (turns.length > 0) {
        turns.forEach(turn => {
          const text = cleanText(turn);
          if (!text) return;

          // User turns typically contain a specific agent identifier
          const isUser = turn.querySelector('[data-message-author-role="user"]') !== null ||
                         turn.querySelector('[class*="user-message"]') !== null;

          messages.push({
            role: isUser ? 'Prompt' : 'Response',
            say: text
          });
        });
      }
    }

    // Strategy 3: Alternating message groups
    if (messages.length === 0) {
      console.log('ChatGPT Exporter: Trying alternating group strategy');

      // Look for the main chat container and find message groups
      const groups = document.querySelectorAll('[class*="group/conversation-turn"], [class*="text-message"]');

      let isUser = true;
      groups.forEach(group => {
        const text = cleanText(group);
        if (!text || text.length < 3) return;

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
    clone.querySelectorAll('svg, button, input, select, nav, header, footer, script, style, [aria-hidden="true"], [class*="icon-"], .sr-only')
      .forEach(e => e.remove());

    return clone.innerText?.trim() || '';
  }

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
    // ChatGPT shows conversation title in the page title or sidebar
    const titleEl = document.querySelector('h1, [class*="conversation-title"], title');
    let title = titleEl?.innerText?.trim() || document.title || 'ChatGPT Conversation';
    title = title.replace(/^ChatGPT\s*[-–—]\s*/i, '').trim();
    return title || 'ChatGPT Conversation';
  }

  function exportConversation() {
    const btn = document.getElementById('chatgpt-export-btn');
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
          powered_by: 'ChatGPT Exporter (custom extension)'
        },
        messages: messages,
        title: title
      };

      const blob = new Blob([JSON.stringify(conversation, null, 2)], { type: 'application/json' });
      const url = URL.createObjectURL(blob);
      const a = document.createElement('a');
      a.href = url;
      a.download = `ChatGPT-${title.replace(/[^a-zA-Z0-9 ]/g, '').substring(0, 50)}.json`;
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
      console.error('ChatGPT Exporter error:', err);
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
      if (!document.getElementById('chatgpt-export-btn')) {
        createExportButton();
      }
    });
    observer.observe(document.body, { childList: true, subtree: true });
  }

  init();
})();
