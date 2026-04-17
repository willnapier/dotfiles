// ------------------------------------------------------------------ //
// Clinical Dashboard — app.js                                        //
// Vanilla JS, no frameworks. Talks to local Axum API on same origin. //
// ------------------------------------------------------------------ //

"use strict";

// --- DOM refs ---

const headerDate     = document.getElementById("header-date");
const clientList     = document.getElementById("client-list");
const refreshBtn     = document.getElementById("refresh-btn");
const clientCard     = document.getElementById("client-card");
const cardClientId   = document.getElementById("card-client-id");
const cardBadge      = document.getElementById("card-badge");
const cardDetails    = document.getElementById("card-details");
const obsSection     = document.getElementById("observation-section");
const obsTextarea    = document.getElementById("observation");
const generateBtn    = document.getElementById("generate-btn");
const noteSection    = document.getElementById("note-section");
const noteStatus     = document.getElementById("note-status");
const noteOutput     = document.getElementById("note-output");
const noteActions    = document.getElementById("note-actions");
const acceptBtn      = document.getElementById("accept-btn");
const editBtn        = document.getElementById("edit-btn");
const rejectBtn      = document.getElementById("reject-btn");
const editSection    = document.getElementById("edit-section");
const editArea       = document.getElementById("edit-area");
const saveEditedBtn  = document.getElementById("save-edited-btn");
const cancelEditBtn  = document.getElementById("cancel-edit-btn");
const emptyState     = document.getElementById("empty-state");
const clientSearch   = document.getElementById("client-search");
const modelSelect    = document.getElementById("model-select");
const compareBtn     = document.getElementById("compare-btn");
const compareSection = document.getElementById("compare-section");
const comparePanels  = document.getElementById("compare-panels");
const clearCompareBtn= document.getElementById("clear-compare-btn");
const toast          = document.getElementById("toast");

// Compare-generate (Q4/Q8 parallel) refs
const compareGenerateBtn     = document.getElementById("compare-generate-btn");
const compareGenerateSection = document.getElementById("compare-generate-section");
const compareCancelBtn       = document.getElementById("compare-cancel-btn");
const compareRejectBtn       = document.getElementById("compare-reject-btn");
const compareEditSection     = document.getElementById("compare-edit-section");
const compareEditLabel       = document.getElementById("compare-edit-label");
const compareEditArea        = document.getElementById("compare-edit-area");
const compareEditSaveBtn     = document.getElementById("compare-edit-save-btn");
const compareEditCancelBtn   = document.getElementById("compare-edit-cancel-btn");

// --- State ---

let selectedClientId = null;
let generatedNote    = "";
let isGenerating     = false;
let compareCount     = 0;

// Compare-generate state
let compareActive    = false;
let compareObservation = "";
// Per-variant generated notes + timing. Set as streams arrive; used for
// save + log-pair call on accept/reject.
const compareState = {
    q4: { note: "", generation_secs: 0, done: false },
    q8: { note: "", generation_secs: 0, done: false },
};
let compareEditingVariant = null;

// Draft observations persist per client (survives client switching + page reload)
const draftKey = (id) => `clinic-draft-${id}`;
const noteKey = (id) => `clinic-note-${id}`;
function saveDraft(id, text) {
    if (id && text.trim()) {
        localStorage.setItem(draftKey(id), text);
    } else if (id) {
        localStorage.removeItem(draftKey(id));
    }
}
function loadDraft(id) {
    return localStorage.getItem(draftKey(id)) || "";
}
function saveGeneratedNote(id, text) {
    if (id && text.trim()) {
        localStorage.setItem(noteKey(id), text);
    } else if (id) {
        localStorage.removeItem(noteKey(id));
    }
}
function loadGeneratedNote(id) {
    return localStorage.getItem(noteKey(id)) || "";
}

// --- Init ---

(function init() {
    // Display today's date
    const now = new Date();
    headerDate.textContent = now.toLocaleDateString("en-GB", {
        weekday: "long",
        year: "numeric",
        month: "long",
        day: "numeric",
    });

    loadAppointments();

    // Wire up events
    refreshBtn.addEventListener("click", loadAppointments);
    generateBtn.addEventListener("click", handleGenerate);
    acceptBtn.addEventListener("click", handleAccept);
    editBtn.addEventListener("click", handleEdit);
    rejectBtn.addEventListener("click", handleReject);
    saveEditedBtn.addEventListener("click", handleSaveEdited);
    cancelEditBtn.addEventListener("click", handleCancelEdit);
    compareBtn.addEventListener("click", handleCompare);
    clearCompareBtn.addEventListener("click", handleClearCompare);

    // Compare-generate (parallel Q4/Q8) wiring
    compareGenerateBtn.addEventListener("click", handleCompareGenerate);
    compareCancelBtn.addEventListener("click", handleCompareCancel);
    compareRejectBtn.addEventListener("click", handleCompareRejectBoth);
    compareEditSaveBtn.addEventListener("click", handleCompareEditSave);
    compareEditCancelBtn.addEventListener("click", handleCompareEditCancel);
    // Per-pane buttons via delegation (data-action + data-variant)
    compareGenerateSection.addEventListener("click", (e) => {
        const btn = e.target.closest("button[data-action]");
        if (!btn) return;
        const variant = btn.dataset.variant;
        const action = btn.dataset.action;
        if (action === "accept") handleCompareAccept(variant);
        else if (action === "edit") handleCompareEdit(variant);
    });

    // Enable generate when observation has content + auto-save draft
    obsTextarea.addEventListener("input", () => {
        const hasText = obsTextarea.value.trim().length > 0;
        generateBtn.disabled = !hasText || isGenerating;
        compareGenerateBtn.disabled = !hasText || isGenerating || compareActive;
        if (selectedClientId) saveDraft(selectedClientId, obsTextarea.value);
    });

    // Client search filter
    clientSearch.addEventListener("input", () => {
        const q = clientSearch.value.trim().toUpperCase();
        for (const li of clientList.children) {
            if (li.classList.contains("placeholder")) continue;
            const id = (li.dataset.id || "").toUpperCase();
            li.style.display = (!q || id.includes(q)) ? "" : "none";
        }
    });

    // Enter in search: select if exactly one match
    clientSearch.addEventListener("keydown", (e) => {
        if (e.key !== "Enter") return;
        e.preventDefault();
        const q = clientSearch.value.trim().toUpperCase();
        if (!q) return;
        const matches = [...clientList.children].filter(li => {
            if (li.classList.contains("placeholder") || !li.dataset.id) return false;
            return li.dataset.id.toUpperCase().includes(q);
        });
        if (matches.length === 1) {
            const id = matches[0].dataset.id;
            clientSearch.value = "";
            // Reset filter to show all
            for (const li of clientList.children) {
                if (!li.classList.contains("placeholder")) li.style.display = "";
            }
            selectClient(id);
        }
    });

    // Ctrl+Enter to generate
    obsTextarea.addEventListener("keydown", (e) => {
        if ((e.ctrlKey || e.metaKey) && e.key === "Enter" && !generateBtn.disabled) {
            handleGenerate();
        }
    });
})();

// --- Data fetching ---

async function loadAppointments() {
    clientList.innerHTML = '<li class="placeholder">Loading&hellip;</li>';
    try {
        const resp = await fetch("/api/today");
        if (!resp.ok) throw new Error(`HTTP ${resp.status}`);
        const appointments = await resp.json();

        if (appointments.length === 0) {
            clientList.innerHTML = '<li class="placeholder">No clients found</li>';
            return;
        }

        clientList.innerHTML = "";
        for (const appt of appointments) {
            const li = document.createElement("li");
            li.dataset.id = appt.client_id;
            if (appt.time) {
                const timeSpan = document.createElement("span");
                timeSpan.className = "client-time";
                timeSpan.textContent = appt.time;
                li.appendChild(timeSpan);
            }
            li.appendChild(document.createTextNode(appt.client_id));
            li.addEventListener("click", () => selectClient(appt.client_id));
            if (appt.client_id === selectedClientId) {
                li.classList.add("active");
            }
            clientList.appendChild(li);
        }
    } catch (err) {
        clientList.innerHTML = '<li class="placeholder">Failed to load</li>';
        showToast("Failed to load appointments: " + err.message, true);
    }
}

async function selectClient(id) {
    // Save current draft before switching
    if (selectedClientId) {
        saveDraft(selectedClientId, obsTextarea.value);
    }

    selectedClientId = id;

    // Highlight in sidebar
    for (const li of clientList.children) {
        li.classList.toggle("active", li.dataset.id === id);
    }

    // Reset workspace
    resetNoteState();
    emptyState.hidden = true;
    clientCard.hidden = false;
    obsSection.hidden = false;

    // Restore draft for this client
    const draft = loadDraft(id);
    obsTextarea.value = draft;
    obsTextarea.focus();
    generateBtn.disabled = draft.trim().length === 0;
    compareGenerateBtn.disabled = draft.trim().length === 0 || compareActive;

    // If a compare session was active for a different client, cancel it
    if (compareActive) compareResetState();

    // Restore generated note if one exists
    const savedNote = loadGeneratedNote(id);
    if (savedNote) {
        generatedNote = savedNote;
        noteSection.hidden = false;
        noteOutput.textContent = savedNote;
        noteStatus.textContent = "Complete";
        noteStatus.className = "status-indicator";
        noteActions.hidden = false;
    }

    // Populate card header
    cardClientId.textContent = id;
    cardBadge.textContent = "";
    cardDetails.innerHTML = '<dt>Loading</dt><dd>&hellip;</dd>';

    try {
        const resp = await fetch(`/api/client/${encodeURIComponent(id)}`);
        if (!resp.ok) throw new Error(`HTTP ${resp.status}`);
        const info = await resp.json();

        cardDetails.innerHTML = "";
        const fields = [
            ["Referrer", info.referrer],
            ["Funding", info.funding],
            ["Sessions", info.session_count],
            ["Modality", info.modality],
        ];
        let anyField = false;
        for (const [label, value] of fields) {
            if (value != null) {
                anyField = true;
                const dt = document.createElement("dt");
                dt.textContent = label;
                const dd = document.createElement("dd");
                dd.textContent = String(value);
                cardDetails.appendChild(dt);
                cardDetails.appendChild(dd);
            }
        }
        if (!anyField) {
            cardDetails.innerHTML = '<dt>Info</dt><dd>No metadata available</dd>';
        }
        if (info.session_count != null) {
            cardBadge.textContent = info.session_count + " sessions";
        }
    } catch (err) {
        cardDetails.innerHTML = '<dt>Error</dt><dd>Could not load client info</dd>';
        showToast("Failed to load client info: " + err.message, true);
    }
}

// --- Note generation ---

async function handleGenerate() {
    if (isGenerating || !selectedClientId) return;
    const observation = obsTextarea.value.trim();
    if (!observation) return;

    isGenerating = true;
    generatedNote = "";
    generateBtn.disabled = true;
    noteSection.hidden = false;
    noteActions.hidden = true;
    noteOutput.textContent = "";
    noteStatus.textContent = "Generating";
    noteStatus.className = "status-indicator streaming";
    editSection.hidden = true;
    const genStartTime = performance.now();

    try {
        const resp = await fetch("/api/note", {
            method: "POST",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify({
                client_id: selectedClientId,
                observation: observation,
                model: modelSelect.value,
            }),
        });

        if (!resp.ok) {
            const errText = await resp.text();
            throw new Error(errText || `HTTP ${resp.status}`);
        }

        const reader = resp.body.getReader();
        const decoder = new TextDecoder();

        while (true) {
            const { done, value } = await reader.read();
            if (done) break;
            const text = decoder.decode(value, { stream: true });
            generatedNote += text;
            noteOutput.textContent = generatedNote;
            // Auto-scroll to bottom
            noteOutput.scrollTop = noteOutput.scrollHeight;
        }

        // Done
        const genElapsed = ((performance.now() - genStartTime) / 1000).toFixed(1);
        noteStatus.textContent = `Complete — ${genElapsed}s`;
        noteStatus.className = "status-indicator";
        noteActions.hidden = false;
        saveGeneratedNote(selectedClientId, generatedNote);
    } catch (err) {
        noteStatus.textContent = "Error";
        noteStatus.className = "status-indicator";
        showToast("Note generation failed: " + err.message, true);
    } finally {
        isGenerating = false;
        generateBtn.disabled = obsTextarea.value.trim().length === 0;
    }
}

// --- Accept / Edit / Reject ---

async function handleAccept() {
    if (!selectedClientId || !generatedNote.trim()) return;

    acceptBtn.disabled = true;
    acceptBtn.textContent = "Saving\u2026";

    try {
        const resp = await fetch("/api/note/save", {
            method: "POST",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify({
                client_id: selectedClientId,
                note: generatedNote,
            }),
        });

        if (!resp.ok) throw new Error(`HTTP ${resp.status}`);
        const result = await resp.json();

        if (result.ok) {
            showToast("Note saved for " + selectedClientId);
            resetNoteState();
            obsTextarea.value = "";
            saveDraft(selectedClientId, "");
            saveGeneratedNote(selectedClientId, "");
            generateBtn.disabled = true;
        } else {
            throw new Error(result.error || "Save failed");
        }
    } catch (err) {
        showToast("Save failed: " + err.message, true);
    } finally {
        acceptBtn.disabled = false;
        acceptBtn.textContent = "Accept & Save";
    }
}

function handleEdit() {
    editSection.hidden = false;
    noteSection.hidden = true;
    editArea.value = generatedNote;
    editArea.focus();
}

function handleSaveEdited() {
    generatedNote = editArea.value;
    noteOutput.textContent = generatedNote;
    editSection.hidden = true;
    noteSection.hidden = false;
    if (selectedClientId) saveGeneratedNote(selectedClientId, generatedNote);
}

function handleCancelEdit() {
    editSection.hidden = true;
    noteSection.hidden = false;
}

function handleReject() {
    if (selectedClientId) saveGeneratedNote(selectedClientId, "");
    resetNoteState();
    obsTextarea.value = "";
    obsTextarea.focus();
    generateBtn.disabled = true;
    showToast("Note rejected");
}

function handleCompare() {
    if (!generatedNote.trim()) return;
    compareCount++;
    const entry = document.createElement("div");
    entry.className = "compare-entry";
    const label = document.createElement("div");
    label.className = "compare-label";
    const modelName = modelSelect.options[modelSelect.selectedIndex].text;
    label.textContent = `#${compareCount} — ${selectedClientId || "?"} — ${modelName} — ${new Date().toLocaleTimeString("en-GB", {hour:"2-digit", minute:"2-digit"})}`;
    const pre = document.createElement("pre");
    pre.textContent = generatedNote;
    entry.appendChild(label);
    entry.appendChild(pre);
    comparePanels.appendChild(entry);
    compareSection.hidden = false;
    showToast("Added to comparison panel");
}

function handleClearCompare() {
    comparePanels.innerHTML = "";
    compareSection.hidden = true;
    compareCount = 0;
}

// --- Compare-generate (Q4/Q8 in parallel) ---

const COMPARE_MODELS = [
    { key: "q4", id: "clinical-voice-q4", label: "Q4" },
    { key: "q8", id: "clinical-voice-q8", label: "Q8" },
];

function compareResetState() {
    compareActive = false;
    compareObservation = "";
    compareState.q4 = { note: "", generation_secs: 0, done: false };
    compareState.q8 = { note: "", generation_secs: 0, done: false };
    compareEditingVariant = null;
    for (const { key } of COMPARE_MODELS) {
        const pane = compareGenerateSection.querySelector(`.compare-pane[data-variant="${key}"]`);
        if (!pane) continue;
        pane.removeAttribute("data-selected");
        pane.querySelector(".compare-pane-output").textContent = "";
        pane.querySelector(".compare-pane-status").textContent = "";
        pane.querySelector(".compare-pane-status").className = "compare-pane-status status-indicator";
        pane.querySelector(".compare-pane-actions").hidden = true;
    }
    compareGenerateSection.querySelector(".compare-global-actions").hidden = true;
    compareGenerateSection.hidden = true;
}

async function streamOneVariant(variantKey, modelId) {
    const pane = compareGenerateSection.querySelector(`.compare-pane[data-variant="${variantKey}"]`);
    const out = pane.querySelector(".compare-pane-output");
    const status = pane.querySelector(".compare-pane-status");

    status.textContent = "Generating";
    status.className = "compare-pane-status status-indicator streaming";
    out.textContent = "";

    const start = performance.now();
    try {
        const resp = await fetch("/api/note", {
            method: "POST",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify({
                client_id: selectedClientId,
                observation: compareObservation,
                model: modelId,
            }),
        });
        if (!resp.ok) {
            const errText = await resp.text();
            throw new Error(errText || `HTTP ${resp.status}`);
        }
        const reader = resp.body.getReader();
        const decoder = new TextDecoder();
        let noteText = "";
        while (true) {
            const { done, value } = await reader.read();
            if (done) break;
            const chunk = decoder.decode(value, { stream: true });
            noteText += chunk;
            out.textContent = noteText;
            out.scrollTop = out.scrollHeight;
        }
        const elapsed = (performance.now() - start) / 1000;
        compareState[variantKey] = { note: noteText, generation_secs: elapsed, done: true };
        status.textContent = `Complete — ${elapsed.toFixed(1)}s`;
        status.className = "compare-pane-status status-indicator";
        pane.querySelector(".compare-pane-actions").hidden = false;
    } catch (err) {
        status.textContent = "Error";
        status.className = "compare-pane-status status-indicator";
        out.textContent = `[${variantKey} failed: ${err.message}]`;
        compareState[variantKey] = { note: "", generation_secs: 0, done: true };
    }
}

async function handleCompareGenerate() {
    if (compareActive || !selectedClientId) return;
    const observation = obsTextarea.value.trim();
    if (!observation) return;

    compareActive = true;
    compareObservation = observation;
    // Hide the single-model generate flow if it's showing
    noteSection.hidden = true;
    editSection.hidden = true;
    compareGenerateSection.hidden = false;
    compareEditSection.hidden = true;

    // Reset any prior compare state (but keep `compareActive=true` above)
    for (const { key } of COMPARE_MODELS) {
        const pane = compareGenerateSection.querySelector(`.compare-pane[data-variant="${key}"]`);
        if (!pane) continue;
        pane.removeAttribute("data-selected");
        pane.querySelector(".compare-pane-output").textContent = "";
        pane.querySelector(".compare-pane-actions").hidden = true;
    }
    compareState.q4 = { note: "", generation_secs: 0, done: false };
    compareState.q8 = { note: "", generation_secs: 0, done: false };

    generateBtn.disabled = true;
    compareGenerateBtn.disabled = true;

    // Fire both streams in parallel. Each writes to its own pane.
    await Promise.all(COMPARE_MODELS.map(m => streamOneVariant(m.key, m.id)));

    // Both done — show the reject-both option
    compareGenerateSection.querySelector(".compare-global-actions").hidden = false;
}

function handleCompareCancel() {
    // User backed out before accepting anything. Do not log.
    compareResetState();
    generateBtn.disabled = obsTextarea.value.trim().length === 0;
    compareGenerateBtn.disabled = obsTextarea.value.trim().length === 0;
    showToast("Comparison cancelled — nothing logged");
}

async function logComparePair(acceptedVariant /* "q4" | "q8" | null */) {
    try {
        const resp = await fetch("/api/note/log-pair", {
            method: "POST",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify({
                client_id: selectedClientId,
                observation: compareObservation,
                q4_note: compareState.q4.note,
                q4_generation_secs: compareState.q4.generation_secs,
                q8_note: compareState.q8.note,
                q8_generation_secs: compareState.q8.generation_secs,
                accepted: acceptedVariant,
            }),
        });
        if (!resp.ok) {
            const errText = await resp.text();
            console.error("log-pair failed:", errText);
        }
    } catch (err) {
        console.error("log-pair error:", err);
    }
}

async function handleCompareAccept(variant) {
    if (!selectedClientId || !compareState[variant].done || !compareState[variant].note.trim()) return;

    // Disable all action buttons
    for (const btn of compareGenerateSection.querySelectorAll(".compare-pane-actions button")) {
        btn.disabled = true;
    }
    compareRejectBtn.disabled = true;

    try {
        // 1) Save the chosen note to the client file
        const saveResp = await fetch("/api/note/save", {
            method: "POST",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify({
                client_id: selectedClientId,
                note: compareState[variant].note,
            }),
        });
        if (!saveResp.ok) throw new Error(`save HTTP ${saveResp.status}`);
        const saveResult = await saveResp.json();
        if (!saveResult.ok) throw new Error(saveResult.error || "Save failed");

        // 2) Log both variants (with accepted flag) to comparisons.jsonl
        await logComparePair(variant);

        showToast(`${variant.toUpperCase()} saved for ${selectedClientId}`);

        // 3) Reset UI
        compareResetState();
        obsTextarea.value = "";
        saveDraft(selectedClientId, "");
        saveGeneratedNote(selectedClientId, "");
        generateBtn.disabled = true;
        compareGenerateBtn.disabled = true;
    } catch (err) {
        showToast("Save failed: " + err.message, true);
        // Re-enable so the user can try again
        for (const btn of compareGenerateSection.querySelectorAll(".compare-pane-actions button")) {
            btn.disabled = false;
        }
        compareRejectBtn.disabled = false;
    }
}

async function handleCompareRejectBoth() {
    if (!confirm("Reject both variants? Both will still be logged to comparisons.jsonl.")) return;
    await logComparePair(null);
    showToast("Both variants rejected (logged)");
    compareResetState();
    generateBtn.disabled = obsTextarea.value.trim().length === 0;
    compareGenerateBtn.disabled = obsTextarea.value.trim().length === 0;
}

function handleCompareEdit(variant) {
    if (!compareState[variant].done || !compareState[variant].note.trim()) return;
    compareEditingVariant = variant;
    compareEditLabel.textContent = variant.toUpperCase();
    compareEditArea.value = compareState[variant].note;
    compareGenerateSection.hidden = true;
    compareEditSection.hidden = false;
    compareEditArea.focus();
}

async function handleCompareEditSave() {
    if (!compareEditingVariant) return;
    // Update stored note with edited text, then accept.
    compareState[compareEditingVariant].note = compareEditArea.value;
    compareEditSection.hidden = true;
    compareGenerateSection.hidden = false;
    // Update the pane display to reflect the edit
    const pane = compareGenerateSection.querySelector(`.compare-pane[data-variant="${compareEditingVariant}"]`);
    if (pane) {
        pane.querySelector(".compare-pane-output").textContent = compareEditArea.value;
    }
    const variant = compareEditingVariant;
    compareEditingVariant = null;
    await handleCompareAccept(variant);
}

function handleCompareEditCancel() {
    compareEditingVariant = null;
    compareEditSection.hidden = true;
    compareGenerateSection.hidden = false;
}

function resetNoteState() {
    noteSection.hidden = true;
    noteActions.hidden = true;
    editSection.hidden = true;
    noteOutput.textContent = "";
    noteStatus.textContent = "";
    noteStatus.className = "status-indicator";
    generatedNote = "";
}

// --- Toast ---

let toastTimer = null;

function showToast(message, isError) {
    toast.textContent = message;
    toast.className = "toast" + (isError ? " error" : "");
    toast.hidden = false;

    if (toastTimer) clearTimeout(toastTimer);
    toastTimer = setTimeout(() => {
        toast.hidden = true;
    }, isError ? 5000 : 3000);
}

// ================================================================== //
// Billing Module — conditionally loaded, invisible when disabled      //
// ================================================================== //

const billingTab        = document.getElementById("billing-tab");
const billingView       = document.getElementById("billing-view");
const billingTbody      = document.getElementById("billing-tbody");
const billingRefreshBtn = document.getElementById("billing-refresh-btn");
const billingBatchBtn   = document.getElementById("billing-batch-btn");
const billingRemindCard = document.getElementById("billing-reminders-card");
const billingRemindList = document.getElementById("billing-reminders-list");
const clinicLayout      = document.querySelector(".layout");
const navTabs           = document.querySelectorAll(".nav-tab");

let billingEnabled = false;
let currentView = "clinic";

// Check billing config on load
(async function checkBilling() {
    try {
        const resp = await fetch("/api/billing/config");
        if (!resp.ok) return;
        const config = await resp.json();
        billingEnabled = config.enabled;
        if (billingEnabled) {
            billingTab.hidden = false;
            // Wire tab switching
            for (const tab of navTabs) {
                tab.addEventListener("click", () => switchView(tab.dataset.view));
            }
            if (billingRefreshBtn) billingRefreshBtn.addEventListener("click", loadBillingData);
            if (billingBatchBtn) billingBatchBtn.addEventListener("click", handleBillingBatch);
        }
    } catch (_) {
        // Billing not available — stay hidden
    }
})();

function switchView(view) {
    currentView = view;
    for (const tab of navTabs) {
        tab.classList.toggle("active", tab.dataset.view === view);
    }
    if (view === "billing") {
        clinicLayout.hidden = true;
        billingView.hidden = false;
        loadBillingData();
    } else {
        clinicLayout.hidden = false;
        billingView.hidden = true;
    }
}

async function loadBillingData() {
    billingTbody.innerHTML = '<tr><td colspan="7" class="placeholder">Loading&hellip;</td></tr>';
    try {
        const [invResp, remResp] = await Promise.all([
            fetch("/api/billing/invoices"),
            fetch("/api/billing/reminders"),
        ]);
        if (!invResp.ok) throw new Error("Failed to load invoices");
        const invoices = await invResp.json();
        const reminders = remResp.ok ? await remResp.json() : [];

        renderInvoiceTable(invoices);
        renderReminders(reminders);
    } catch (err) {
        billingTbody.innerHTML = '<tr><td colspan="7" class="placeholder">Failed to load</td></tr>';
        showToast("Billing data: " + err.message, true);
    }
}

function renderInvoiceTable(invoices) {
    if (invoices.length === 0) {
        billingTbody.innerHTML = '<tr><td colspan="7" class="placeholder">No outstanding invoices</td></tr>';
        return;
    }

    billingTbody.innerHTML = "";
    for (const inv of invoices) {
        const tr = document.createElement("tr");

        const stateClass = inv.state === "overdue" ? "state-overdue"
            : inv.state === "sent" ? "state-sent" : "state-draft";

        tr.innerHTML = `
            <td><code>${esc(inv.reference)}</code></td>
            <td>${esc(inv.client_id)}</td>
            <td>${esc(inv.bill_to)}</td>
            <td class="amount">${esc(inv.currency)} ${inv.total.toFixed(0)}</td>
            <td>${esc(inv.due_date)}${inv.days_overdue > 0 ? ` <small>(${inv.days_overdue}d)</small>` : ""}</td>
            <td class="${stateClass}">${esc(inv.state)}</td>
            <td class="actions-cell"></td>
        `;

        const actionsCell = tr.querySelector(".actions-cell");

        const paidBtn = document.createElement("button");
        paidBtn.className = "btn btn-accept";
        paidBtn.textContent = "Paid";
        paidBtn.addEventListener("click", () => markPaid(inv.reference));
        actionsCell.appendChild(paidBtn);

        const cancelBtn = document.createElement("button");
        cancelBtn.className = "btn btn-reject";
        cancelBtn.textContent = "Cancel";
        cancelBtn.addEventListener("click", () => cancelInvoice(inv.reference));
        actionsCell.appendChild(cancelBtn);

        billingTbody.appendChild(tr);
    }
}

function renderReminders(reminders) {
    if (reminders.length === 0) {
        billingRemindCard.hidden = true;
        return;
    }

    billingRemindCard.hidden = false;
    billingRemindList.innerHTML = "";

    for (const rem of reminders) {
        const entry = document.createElement("div");
        entry.className = "reminder-entry";

        const header = document.createElement("div");
        header.className = "reminder-header";

        const badge = document.createElement("span");
        badge.className = `tone-badge tone-${rem.tone}`;
        badge.textContent = rem.tone;

        const subj = document.createElement("span");
        subj.className = "reminder-subject";
        subj.textContent = rem.subject;

        header.appendChild(badge);
        header.appendChild(subj);

        const body = document.createElement("div");
        body.className = "reminder-body";
        body.textContent = rem.body;

        entry.appendChild(header);
        entry.appendChild(body);
        billingRemindList.appendChild(entry);
    }
}

async function markPaid(reference) {
    try {
        const resp = await fetch("/api/billing/paid", {
            method: "POST",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify({ reference }),
        });
        if (!resp.ok) throw new Error("Failed");
        showToast(`${reference} marked as paid`);
        loadBillingData();
    } catch (err) {
        showToast("Mark paid failed: " + err.message, true);
    }
}

async function cancelInvoice(reference) {
    try {
        const resp = await fetch("/api/billing/cancel", {
            method: "POST",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify({ reference }),
        });
        if (!resp.ok) throw new Error("Failed");
        showToast(`${reference} cancelled`);
        loadBillingData();
    } catch (err) {
        showToast("Cancel failed: " + err.message, true);
    }
}

async function handleBillingBatch() {
    billingBatchBtn.disabled = true;
    billingBatchBtn.textContent = "Creating\u2026";
    try {
        // Get all clients and try to invoice each
        const resp = await fetch("/api/clients");
        if (!resp.ok) throw new Error("Failed to list clients");
        const clients = await resp.json();

        let created = 0;
        for (const client of clients) {
            const invResp = await fetch(`/api/billing/invoice/${encodeURIComponent(client.id)}`, {
                method: "POST",
            });
            if (invResp.ok) {
                const result = await invResp.json();
                if (result.created) created++;
            }
        }

        if (created > 0) {
            showToast(`${created} invoice(s) created`);
        } else {
            showToast("No uninvoiced sessions found");
        }
        loadBillingData();
    } catch (err) {
        showToast("Batch invoice failed: " + err.message, true);
    } finally {
        billingBatchBtn.disabled = false;
        billingBatchBtn.textContent = "Invoice All Uninvoiced";
    }
}

function esc(str) {
    const div = document.createElement("div");
    div.textContent = str || "";
    return div.innerHTML;
}
