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
const toast          = document.getElementById("toast");

// --- State ---

let selectedClientId = null;
let generatedNote    = "";
let isGenerating     = false;

// Draft observations persist per client (survives client switching + page reload)
const draftKey = (id) => `clinic-draft-${id}`;
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

    // Enable generate when observation has content + auto-save draft
    obsTextarea.addEventListener("input", () => {
        generateBtn.disabled = obsTextarea.value.trim().length === 0 || isGenerating;
        if (selectedClientId) saveDraft(selectedClientId, obsTextarea.value);
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

    try {
        const resp = await fetch("/api/note", {
            method: "POST",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify({
                client_id: selectedClientId,
                observation: observation,
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
        noteStatus.textContent = "Complete";
        noteStatus.className = "status-indicator";
        noteActions.hidden = false;
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
            saveDraft(selectedClientId, "");  // clear draft on save
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
}

function handleCancelEdit() {
    editSection.hidden = true;
    noteSection.hidden = false;
}

function handleReject() {
    resetNoteState();
    obsTextarea.value = "";
    obsTextarea.focus();
    generateBtn.disabled = true;
    showToast("Note rejected");
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
