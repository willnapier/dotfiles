//! Clinical Dashboard — The Product prototype
//! Pure Rust, GPU-rendered via wgpu. No WebView, no browser.
//! Owns its own clinic state (session file, attendance tracking).
//!
//! Keyboard navigation:
//! - Tab / Shift+Tab: cycle focus zones (Search → Client List → Observation → Note)
//! - Arrow Up/Down: navigate client list when list zone is active
//! - Enter: select highlighted client (list zone) or submit search (search zone)
//! - Escape: return to client list zone
//! - Ctrl+K: jump to search

use iced::keyboard::{self, key};
use iced::widget::{
    button, column, container, pick_list, row, rule, scrollable, text,
    text_editor, text_input, Column,
};
use iced::widget::operation;
use iced::{color, Element, Font, Length, Subscription, Task, Theme};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

fn main() -> iced::Result {
    iced::application(App::boot, App::update, App::view)
        .subscription(App::subscription)
        .title("Clinical Dashboard")
        .theme(app_theme)
        .window(iced::window::Settings {
            size: iced::Size::new(1100.0, 750.0),
            platform_specific: iced::window::settings::PlatformSpecific {
                title_hidden: true,
                titlebar_transparent: true,
                fullsize_content_view: true,
            },
            ..Default::default()
        })
        .run()
}

fn app_theme(_state: &App) -> Theme {
    Theme::SolarizedDark
}

// ---------------------------------------------------------------------------
// Focus zone system
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FocusZone {
    SearchBox,
    ClientList,
    ObservationEditor,
    NoteEditor,
}

const SEARCH_ID: &str = "search-input";
const OBS_EDITOR_ID: &str = "obs-editor";
const NOTE_EDITOR_ID: &str = "note-editor";
const CLIENT_SCROLL_ID: &str = "client-scroll";

fn focus_zone_task(zone: FocusZone) -> Task<Msg> {
    match zone {
        FocusZone::SearchBox => {
            operation::focus(SEARCH_ID)
        }
        FocusZone::ClientList => {
            // Focus a non-existent widget ID to unfocus everything.
            // The focus operation traverses all Focusable widgets and
            // unfocuses any that don't match the target.
            operation::focus("__unfocus_sentinel__")
        }
        FocusZone::ObservationEditor => {
            operation::focus(OBS_EDITOR_ID)
        }
        FocusZone::NoteEditor => {
            operation::focus(NOTE_EDITOR_ID)
        }
    }
}

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
enum ModelChoice { Q4, Q8 }

impl std::fmt::Display for ModelChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self { ModelChoice::Q4 => write!(f, "Q4"), ModelChoice::Q8 => write!(f, "Q8") }
    }
}
impl ModelChoice {
    fn model_name(&self) -> &str {
        match self { ModelChoice::Q4 => "clinical-voice-q4", ModelChoice::Q8 => "clinical-voice-q8" }
    }
    const ALL: &'static [ModelChoice] = &[ModelChoice::Q4, ModelChoice::Q8];
}

#[derive(Debug, Clone)]
struct ClientEntry { id: String }

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum ClinicStatus { Pending, Done, Dna, Cancelled }

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ClinicClient {
    id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    time: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    end_time: Option<String>,
    status: ClinicStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    rate_tag: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    draft_observation: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ClinicSession {
    date: String,
    started_at: String,
    clients: Vec<ClinicClient>,
}

// ---------------------------------------------------------------------------
// Paths + persistence
// ---------------------------------------------------------------------------

fn home() -> PathBuf {
    dirs::home_dir().expect("no home dir")
}

/// Clinical data root. Reads `[paths] clinical_root` from
/// `~/.config/clinical-product/config.toml` (or `voice-config.toml`).
/// Falls back to `~/Clinical`.
fn clinical_root() -> PathBuf {
    let config_dir = home().join(".config").join("clinical-product");
    let config_path = if config_dir.join("config.toml").exists() {
        config_dir.join("config.toml")
    } else {
        config_dir.join("voice-config.toml")
    };
    if let Ok(data) = std::fs::read_to_string(&config_path) {
        if let Ok(val) = data.parse::<toml::Value>() {
            if let Some(root) = val.get("paths")
                .and_then(|p| p.get("clinical_root"))
                .and_then(|v| v.as_str())
            {
                if root.starts_with("~/") {
                    return home().join(&root[2..]);
                }
                return PathBuf::from(root);
            }
        }
    }
    home().join("Clinical")
}

fn clients_dir() -> PathBuf { clinical_root().join("clients") }
fn attendance_dir() -> PathBuf { clinical_root().join("attendance") }

fn session_dir() -> PathBuf {
    home().join(".local/share/clinical-dashboard")
}

fn session_path(date: &str) -> PathBuf {
    session_dir().join(format!("session-{date}.json"))
}

fn load_session(date: &str) -> Option<ClinicSession> {
    let path = session_path(date);
    let data = std::fs::read_to_string(&path).ok()?;
    let mut session: ClinicSession = serde_json::from_str(&data).ok()?;
    // Sort clients by start time (empty time sorts last)
    session.clients.sort_by(|a, b| {
        let ta = a.time.as_deref().unwrap_or("99:99");
        let tb = b.time.as_deref().unwrap_or("99:99");
        ta.cmp(tb)
    });
    Some(session)
}

fn save_session(session: &ClinicSession) {
    let _ = std::fs::create_dir_all(session_dir());
    let path = session_path(&session.date);
    if let Ok(json) = serde_json::to_string_pretty(session) {
        let _ = std::fs::write(path, json);
    }
}

fn load_clients() -> Vec<ClientEntry> {
    let mut v = Vec::new();
    if let Ok(entries) = std::fs::read_dir(clients_dir()) {
        for e in entries.flatten() {
            if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                if let Some(n) = e.file_name().to_str() {
                    if !n.starts_with('.') { v.push(ClientEntry { id: n.to_string() }); }
                }
            }
        }
    }
    v.sort_by(|a, b| a.id.cmp(&b.id));
    v
}

fn filter(clients: &[ClientEntry], q: &str) -> Vec<ClientEntry> {
    let q = q.to_uppercase();
    clients.iter().filter(|c| q.is_empty() || c.id.to_uppercase().contains(&q)).cloned().collect()
}

fn generate_attendance_report(session: &ClinicSession) -> String {
    let date = chrono::NaiveDate::parse_from_str(&session.date, "%Y-%m-%d")
        .map(|d| d.format("%a %-d %b").to_string())
        .unwrap_or_else(|_| session.date.clone());

    let mut lines = vec![format!("{date} — Attendance"), String::new()];
    let mut attended = 0u32;
    let mut dna = 0u32;
    let mut insurer = 0u32;

    for c in &session.clients {
        if c.status == ClinicStatus::Cancelled { continue; }
        let marker = match c.status {
            ClinicStatus::Done => { attended += 1; "✓" }
            ClinicStatus::Dna => { dna += 1; "✗" }
            _ => "?"
        };
        if c.rate_tag.as_deref() == Some("insurer") { insurer += 1; }
        let time = c.time.as_deref().unwrap_or("");
        let tag = c.rate_tag.as_deref().unwrap_or("");
        lines.push(format!("{marker} {} {time} {tag}", c.id).trim_end().to_string());
    }

    let total = attended + dna;
    lines.push(String::new());
    let mut summary = format!("{attended}/{total} attended");
    if dna > 0 { summary.push_str(&format!(" · {dna} DNA/LC")); }
    if insurer > 0 { summary.push_str(&format!(" · {insurer} insurer")); }
    lines.push(summary);
    lines.join("\n")
}

// ---------------------------------------------------------------------------
// Async commands
// ---------------------------------------------------------------------------

async fn gen(id: String, obs: String, model: String) -> (String, f64) {
    let t = std::time::Instant::now();
    let mut cmd = tokio::process::Command::new("clinical");
    cmd.arg("note").arg(&id).arg(&obs).arg("--no-save").arg("--yes");
    if !model.is_empty() { cmd.arg("--model-override").arg(&model); }
    cmd.stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
    let r = match cmd.spawn() {
        Ok(c) => match c.wait_with_output().await {
            Ok(o) => String::from_utf8_lossy(&o.stdout).to_string(),
            Err(e) => format!("[error: {e}]"),
        },
        Err(e) => format!("[error: {e}]"),
    };
    (r, t.elapsed().as_secs_f64())
}

async fn do_save(id: String, note: String) -> Result<String, String> {
    let mut c = tokio::process::Command::new("clinical")
        .arg("note-save").arg(&id)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn().map_err(|e| e.to_string())?;
    if let Some(mut s) = c.stdin.take() {
        use tokio::io::AsyncWriteExt;
        s.write_all(note.as_bytes()).await.map_err(|e| e.to_string())?;
    }
    let o = c.wait_with_output().await.map_err(|e| e.to_string())?;
    if o.status.success() { Ok(String::from_utf8_lossy(&o.stdout).to_string()) }
    else { Err(String::from_utf8_lossy(&o.stderr).to_string()) }
}

async fn check_inference() -> bool {
    match tokio::process::Command::new("curl")
        .args(["-s", "--max-time", "3", "http://localhost:11434/api/tags"])
        .stdout(std::process::Stdio::piped())
        .spawn()
    {
        Ok(c) => match c.wait_with_output().await {
            Ok(o) => o.status.success() && !o.stdout.is_empty(),
            Err(_) => false,
        },
        Err(_) => false,
    }
}

async fn restart_inference() -> bool {
    eprintln!("Inference down — running inference-start...");
    match tokio::process::Command::new("inference-start")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(c) => match c.wait_with_output().await {
            Ok(o) => {
                let ok = o.status.success();
                if !ok {
                    let stderr = String::from_utf8_lossy(&o.stderr);
                    eprintln!("inference-start failed: {stderr}");
                }
                ok
            }
            Err(e) => { eprintln!("inference-start error: {e}"); false }
        },
        Err(e) => { eprintln!("Failed to spawn inference-start: {e}"); false }
    }
}

// ---------------------------------------------------------------------------
// Keyboard event mapping
// ---------------------------------------------------------------------------

fn map_keyboard_event(event: keyboard::Event) -> Option<Msg> {
    match event {
        keyboard::Event::KeyPressed { key, modifiers, .. } => {
            match key {
                keyboard::Key::Named(key::Named::Tab) => {
                    Some(Msg::TabPressed(modifiers.shift()))
                }
                keyboard::Key::Named(key::Named::ArrowUp) => Some(Msg::ArrowUp),
                keyboard::Key::Named(key::Named::ArrowDown) => Some(Msg::ArrowDown),
                keyboard::Key::Named(key::Named::Enter) => Some(Msg::EnterPressed),
                keyboard::Key::Named(key::Named::Escape) => Some(Msg::EscapePressed),
                keyboard::Key::Character(ref c) if c.as_str() == "k" && modifiers.command() => {
                    Some(Msg::FocusSearch)
                }
                // Window close: Cmd+W / Cmd+Q (macOS), Ctrl+W / Ctrl+Q (Linux/Windows)
                keyboard::Key::Character(ref c)
                    if (c.as_str() == "w" || c.as_str() == "q") && modifiers.command() =>
                {
                    Some(Msg::CloseWindow)
                }
                _ => None,
            }
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Styling
// ---------------------------------------------------------------------------

/// Container style for the active focus zone ring.
fn focus_ring_style(_theme: &Theme) -> container::Style {
    container::Style {
        border: iced::Border {
            color: color!(0x2aa198),  // solarized cyan
            width: 2.0,
            radius: 4.0.into(),
        },
        ..Default::default()
    }
}

/// Sidebar background
fn sidebar_style(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(iced::Background::Color(color!(0x002b36))),
        ..Default::default()
    }
}

/// Header bar background
fn header_style(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(iced::Background::Color(color!(0x002b36))),
        ..Default::default()
    }
}

/// Highlighted client item (keyboard selection) background
fn highlight_style(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(iced::Background::Color(color!(0x073642))),
        border: iced::Border {
            color: color!(0x2aa198),
            width: 1.0,
            radius: 2.0.into(),
        },
        ..Default::default()
    }
}

// ---------------------------------------------------------------------------
// App
// ---------------------------------------------------------------------------

struct App {
    clients: Vec<ClientEntry>,
    filtered: Vec<ClientEntry>,
    search: String,
    selected: Option<String>,
    obs: text_editor::Content,
    model: ModelChoice,
    note: text_editor::Content,
    note_text: String,
    status: String,
    busy: bool,
    show_note: bool,
    compares: Vec<(String, String)>,
    highlight: usize,
    // Focus management
    focus_zone: FocusZone,
    // Clinic session state
    last_removed: Option<ClinicClient>,
    session: ClinicSession,
    session_start: std::time::Instant,
    inference_ok: bool,
    inference_reconnecting: bool,
    add_client_input: String,
    clinic_ended: bool,
    // Date navigation
    viewing_date: chrono::NaiveDate,
}

#[derive(Debug, Clone)]
enum Msg {
    Search(String),
    Select(String),
    Obs(text_editor::Action),
    NoteEdit(text_editor::Action),
    Model(ModelChoice),
    Gen,
    GenDone(String, f64),
    Accept,
    Saved(Result<String, String>),
    Reject,
    Compare,
    ClearCmp,
    // Clinic workflow
    MarkDna(String),
    MarkCancelled(String),
    RemoveFromClinic(String),
    UndoRemove,
    AddClientInput(String),
    AddClient,
    EndClinic,
    InferenceChecked(bool),
    InferenceHeartbeat,
    InferenceRestarted(bool),
    // Date navigation
    PrevDay,
    NextDay,
    GoToday,
    // Keyboard navigation
    TabPressed(bool),  // shift held?
    ArrowUp,
    ArrowDown,
    EnterPressed,
    EscapePressed,
    FocusSearch,
    CloseWindow,
    WindowId(Option<iced::window::Id>),
    NoOp,
}

impl App {
    fn boot() -> (Self, Task<Msg>) {
        let clients = load_clients();
        let filtered = clients.clone();
        let viewing_date = chrono::Local::now().date_naive();
        let today = viewing_date.format("%Y-%m-%d").to_string();

        let session = load_session(&today).unwrap_or_else(|| ClinicSession {
            date: today.clone(),
            started_at: chrono::Local::now().to_rfc3339(),
            clients: Vec::new(),
        });

        (Self {
            clients, filtered, search: String::new(), selected: None,
            obs: text_editor::Content::new(), model: ModelChoice::Q4,
            note: text_editor::Content::new(), note_text: String::new(),
            status: String::new(), busy: false,
            show_note: false, compares: Vec::new(), highlight: 0,
            focus_zone: FocusZone::ClientList,
            last_removed: None,
            session, session_start: std::time::Instant::now(),
            inference_ok: false, inference_reconnecting: false, add_client_input: String::new(),
            clinic_ended: false, viewing_date,
        }, Task::perform(check_inference(), Msg::InferenceChecked))
    }

    fn persist_session(&self) {
        save_session(&self.session);
    }

    fn session_client_status(&self, id: &str) -> Option<&ClinicStatus> {
        self.session.clients.iter().find(|c| c.id == id).map(|c| &c.status)
    }

    fn all_resolved(&self) -> bool {
        !self.session.clients.is_empty()
            && self.session.clients.iter().all(|c| c.status != ClinicStatus::Pending)
    }

    /// The items currently visible in the sidebar list.
    /// If search is active, shows search results; otherwise shows clinic list.
    fn visible_list_len(&self) -> usize {
        if !self.search.is_empty() {
            self.filtered.len()
        } else {
            self.session.clients.len()
        }
    }

    /// Get the client ID at the given highlight index.
    fn client_at_highlight(&self) -> Option<String> {
        if !self.search.is_empty() {
            self.filtered.get(self.highlight).map(|c| c.id.clone())
        } else {
            self.session.clients.get(self.highlight).map(|c| c.id.clone())
        }
    }

    /// Available focus zones given current UI state.
    fn available_zones(&self) -> Vec<FocusZone> {
        let mut zones = vec![FocusZone::SearchBox, FocusZone::ClientList];
        if self.selected.is_some() {
            zones.push(FocusZone::ObservationEditor);
        }
        if self.show_note {
            zones.push(FocusZone::NoteEditor);
        }
        zones
    }

    fn switch_date(&mut self, date: chrono::NaiveDate) {
        self.viewing_date = date;
        let date_str = date.format("%Y-%m-%d").to_string();
        self.session = load_session(&date_str).unwrap_or_else(|| ClinicSession {
            date: date_str,
            started_at: chrono::Local::now().to_rfc3339(),
            clients: Vec::new(),
        });
        self.selected = None;
        self.obs = text_editor::Content::new();
        self.note = text_editor::Content::new();
        self.note_text.clear();
        self.show_note = false;
        self.status.clear();
        self.clinic_ended = false;
    }

    fn update(&mut self, msg: Msg) -> Task<Msg> {
        match msg {
            Msg::InferenceChecked(ok) => {
                self.inference_ok = ok;
                if !ok && !self.inference_reconnecting {
                    // Auto-reconnect
                    self.inference_reconnecting = true;
                    Task::perform(restart_inference(), Msg::InferenceRestarted)
                } else {
                    if ok { self.inference_reconnecting = false; }
                    Task::none()
                }
            }

            Msg::InferenceHeartbeat => {
                Task::perform(check_inference(), Msg::InferenceChecked)
            }

            Msg::InferenceRestarted(ok) => {
                self.inference_reconnecting = false;
                self.inference_ok = ok;
                Task::none()
            }

            Msg::Search(q) => {
                self.search = q;
                self.filtered = filter(&self.clients, &self.search);
                self.highlight = 0;
                Task::none()
            }

            Msg::Select(id) => {
                // Load draft observation if one exists for this client
                let draft = self.session.clients.iter()
                    .find(|c| c.id == id)
                    .and_then(|c| c.draft_observation.clone());
                self.selected = Some(id);
                self.obs = match draft {
                    Some(ref d) if !d.is_empty() => text_editor::Content::with_text(d),
                    _ => text_editor::Content::new(),
                };
                self.note = text_editor::Content::new();
                self.note_text.clear();
                self.show_note = false;
                self.status = match draft {
                    Some(ref d) if !d.is_empty() => "Draft restored".into(),
                    _ => String::new(),
                };
                // Auto-switch to observation editor after selecting a client
                self.focus_zone = FocusZone::ObservationEditor;
                focus_zone_task(FocusZone::ObservationEditor)
            }

            Msg::Obs(a) => {
                let old_text = self.obs.text();
                self.obs.perform(a);
                let new_text = self.obs.text();
                // Only persist when text actually changed (not on cursor/selection moves)
                if old_text != new_text {
                    if let Some(ref id) = self.selected {
                        let draft = if new_text.trim().is_empty() { None } else { Some(new_text) };
                        if let Some(c) = self.session.clients.iter_mut().find(|c| c.id == *id) {
                            c.draft_observation = draft;
                        } else if draft.is_some() {
                            self.session.clients.push(ClinicClient {
                                id: id.clone(), time: None, end_time: None,
                                status: ClinicStatus::Pending, rate_tag: None,
                                draft_observation: draft,
                            });
                        }
                        self.persist_session();
                    }
                }
                Task::none()
            }
            Msg::NoteEdit(a) => {
                self.note.perform(a);
                self.note_text = self.note.text();
                Task::none()
            }
            Msg::Model(m) => { self.model = m; Task::none() }

            Msg::Gen => {
                if !self.inference_ok {
                    self.status = "Inference not connected — run inference-start".into();
                    return Task::none();
                }
                let Some(ref id) = self.selected else { return Task::none() };
                let t = self.obs.text();
                if t.trim().is_empty() { return Task::none() }
                self.busy = true;
                self.show_note = true;
                self.note = text_editor::Content::new();
                self.note_text.clear();
                self.status = "Generating...".into();
                let id = id.clone();
                let m = self.model.model_name().to_string();
                Task::perform(gen(id, t, m), |(n, s)| Msg::GenDone(n, s))
            }

            Msg::GenDone(n, s) => {
                self.note = text_editor::Content::with_text(&n);
                self.note_text = n;
                self.status = format!("Complete — {s:.1}s");
                self.busy = false;
                // Focus the generated note for review
                self.focus_zone = FocusZone::NoteEditor;
                focus_zone_task(FocusZone::NoteEditor)
            }

            Msg::Accept => {
                let Some(ref id) = self.selected else { return Task::none() };
                let id = id.clone();
                let n = self.note_text.clone();
                Task::perform(do_save(id, n), Msg::Saved)
            }

            Msg::Saved(r) => {
                match r {
                    Ok(_) => {
                        let id = self.selected.clone().unwrap_or_default();
                        self.status = format!("Saved for {id}");
                        self.obs = text_editor::Content::new();
                        self.note = text_editor::Content::new();
                        self.note_text.clear();
                        self.show_note = false;
                        // Auto-mark done in session and clear draft
                        if let Some(c) = self.session.clients.iter_mut().find(|c| c.id == id) {
                            c.status = ClinicStatus::Done;
                            c.draft_observation = None;
                        } else {
                            self.session.clients.push(ClinicClient {
                                id: id.clone(), time: None, end_time: None, status: ClinicStatus::Done, rate_tag: None, draft_observation: None,
                            });
                        }
                        self.persist_session();
                        // Return to client list
                        self.focus_zone = FocusZone::ClientList;
                        focus_zone_task(FocusZone::ClientList)
                    }
                    Err(e) => { self.status = format!("Failed: {e}"); Task::none() }
                }
            }

            Msg::Reject => {
                self.note = text_editor::Content::new();
                self.note_text.clear();
                self.show_note = false;
                self.obs = text_editor::Content::new();
                self.status.clear();
                self.focus_zone = FocusZone::ClientList;
                focus_zone_task(FocusZone::ClientList)
            }

            Msg::Compare => {
                if !self.note_text.is_empty() {
                    let l = format!("{} — {}", self.selected.as_deref().unwrap_or("?"), self.model);
                    self.compares.push((l, self.note_text.clone()));
                }
                Task::none()
            }
            Msg::ClearCmp => { self.compares.clear(); Task::none() }

            Msg::MarkDna(id) => {
                if let Some(c) = self.session.clients.iter_mut().find(|c| c.id == id) {
                    c.status = ClinicStatus::Dna;
                }
                self.persist_session();
                Task::none()
            }
            Msg::MarkCancelled(id) => {
                if let Some(c) = self.session.clients.iter_mut().find(|c| c.id == id) {
                    c.status = ClinicStatus::Cancelled;
                }
                self.persist_session();
                Task::none()
            }

            Msg::RemoveFromClinic(id) => {
                // Stash for undo
                self.last_removed = self.session.clients.iter()
                    .find(|c| c.id == id).cloned();
                self.session.clients.retain(|c| c.id != id);
                self.persist_session();
                if self.selected.as_deref() == Some(&id) {
                    self.selected = None;
                    self.obs = text_editor::Content::new();
                    self.note = text_editor::Content::new();
                    self.note_text.clear();
                    self.show_note = false;
                }
                self.status = format!("{id} removed — click Undo to restore");
                self.focus_zone = FocusZone::ClientList;
                focus_zone_task(FocusZone::ClientList)
            }

            Msg::UndoRemove => {
                if let Some(client) = self.last_removed.take() {
                    let id = client.id.clone();
                    self.session.clients.push(client);
                    // Re-sort by time
                    self.session.clients.sort_by(|a, b| {
                        let ta = a.time.as_deref().unwrap_or("99:99");
                        let tb = b.time.as_deref().unwrap_or("99:99");
                        ta.cmp(tb)
                    });
                    self.persist_session();
                    self.status = format!("{id} restored");
                }
                Task::none()
            }

            Msg::AddClientInput(s) => { self.add_client_input = s; Task::none() }
            Msg::AddClient => {
                let id = self.add_client_input.trim().to_uppercase();
                if !id.is_empty() && !self.session.clients.iter().any(|c| c.id == id) {
                    self.session.clients.push(ClinicClient {
                        id, time: None, end_time: None, status: ClinicStatus::Pending, rate_tag: None, draft_observation: None,
                    });
                    self.persist_session();
                }
                self.add_client_input.clear();
                Task::none()
            }

            Msg::EndClinic => {
                let report = generate_attendance_report(&self.session);
                let _ = std::fs::create_dir_all(attendance_dir());
                let report_path = attendance_dir().join(format!("{}.txt", self.session.date));
                let _ = std::fs::write(&report_path, &report);

                let elapsed = self.session_start.elapsed().as_secs() / 60;
                let done: Vec<_> = self.session.clients.iter()
                    .filter(|c| c.status == ClinicStatus::Done)
                    .map(|c| c.id.clone())
                    .collect();
                let entry = format!("clinic:: {} clients {}min - {}", done.len(), elapsed, done.join(", "));
                let _ = std::process::Command::new("daypage-append").arg(&entry).spawn();

                self.status = format!("Clinic ended. Report saved. {} clients documented.", done.len());
                self.clinic_ended = true;
                Task::none()
            }

            // ---------------------------------------------------------------
            // Date navigation
            // ---------------------------------------------------------------

            Msg::PrevDay => {
                let prev = self.viewing_date - chrono::Duration::days(1);
                self.switch_date(prev);
                Task::none()
            }
            Msg::NextDay => {
                let next = self.viewing_date + chrono::Duration::days(1);
                self.switch_date(next);
                Task::none()
            }
            Msg::GoToday => {
                self.switch_date(chrono::Local::now().date_naive());
                Task::none()
            }

            // ---------------------------------------------------------------
            // Keyboard navigation
            // ---------------------------------------------------------------

            Msg::TabPressed(shift) => {
                let zones = self.available_zones();
                let current_idx = zones.iter().position(|z| *z == self.focus_zone).unwrap_or(0);
                let next_idx = if shift {
                    if current_idx == 0 { zones.len() - 1 } else { current_idx - 1 }
                } else {
                    (current_idx + 1) % zones.len()
                };
                self.focus_zone = zones[next_idx];
                focus_zone_task(self.focus_zone)
            }

            Msg::ArrowUp => {
                // Only reaches subscription when no text widget has focus
                // (text editors capture arrow keys internally)
                if self.focus_zone == FocusZone::ClientList {
                    let len = self.visible_list_len();
                    if len > 0 && self.highlight > 0 {
                        self.highlight -= 1;
                    }
                    self.scroll_to_highlight()
                } else {
                    Task::none()
                }
            }

            Msg::ArrowDown => {
                if self.focus_zone == FocusZone::ClientList {
                    let len = self.visible_list_len();
                    if len > 0 && self.highlight < len - 1 {
                        self.highlight += 1;
                    }
                    self.scroll_to_highlight()
                } else {
                    Task::none()
                }
            }

            Msg::EnterPressed => {
                // Only reaches subscription when no text widget has focus
                match self.focus_zone {
                    FocusZone::ClientList => {
                        if let Some(id) = self.client_at_highlight() {
                            self.update(Msg::Select(id))
                        } else {
                            Task::none()
                        }
                    }
                    FocusZone::SearchBox => {
                        // Select the first filtered result
                        if self.filtered.len() == 1 {
                            let id = self.filtered[0].id.clone();
                            self.update(Msg::Select(id))
                        } else {
                            Task::none()
                        }
                    }
                    _ => Task::none(),
                }
            }

            Msg::EscapePressed => {
                // "Go back" — deselect client and return to client list
                if self.selected.is_some() {
                    self.selected = None;
                    self.obs = text_editor::Content::new();
                    self.note = text_editor::Content::new();
                    self.note_text.clear();
                    self.show_note = false;
                    self.status.clear();
                }
                self.focus_zone = FocusZone::ClientList;
                focus_zone_task(FocusZone::ClientList)
            }

            Msg::FocusSearch => {
                self.focus_zone = FocusZone::SearchBox;
                focus_zone_task(FocusZone::SearchBox)
            }

            Msg::CloseWindow => {
                iced::window::oldest().map(Msg::WindowId)
            }
            Msg::WindowId(Some(id)) => iced::window::close(id),
            Msg::WindowId(None) => Task::none(),

            Msg::NoOp => Task::none(),
        }
    }

    /// Scroll the client list to keep the highlighted item visible.
    /// Each item is roughly 22px (12px text + 6px padding + spacing).
    fn scroll_to_highlight(&self) -> Task<Msg> {
        let len = self.visible_list_len();
        if len == 0 { return Task::none(); }
        // Approximate pixel offset: item_height * highlight_index,
        // clamped so the item appears near the top of the visible area.
        let item_height: f32 = 22.0;
        let y = (self.highlight as f32) * item_height;
        operation::snap_to(
            CLIENT_SCROLL_ID,
            operation::RelativeOffset::START,
        )
        .chain(operation::scroll_to(
            CLIENT_SCROLL_ID,
            operation::AbsoluteOffset { x: 0.0, y },
        ))
    }

    fn view(&self) -> Element<'_, Msg> {
        let is_today = self.viewing_date == chrono::Local::now().date_naive();
        let date_display = self.viewing_date.format("%A %d %B %Y").to_string();

        // Date navigation bar
        let date_nav = row![
            button(text("◀").size(14)).on_press(Msg::PrevDay).padding([3, 10]).style(button::text),
            if is_today {
                text(date_display.clone()).size(14).color(color!(0xfdf6e3))
            } else {
                text(date_display.clone()).size(14).color(color!(0xd4a020))
            },
            button(text("▶").size(14)).on_press(Msg::NextDay).padding([3, 10]).style(button::text),
            if !is_today {
                Element::from(button(text("Today").size(12)).on_press(Msg::GoToday).padding([3, 8]).style(button::secondary))
            } else {
                Element::from(iced::widget::Space::new().width(0))
            },
        ].spacing(4).align_y(iced::Alignment::Center);

        // Header
        let mut hdr_row = row![
            iced::widget::Space::new().width(70),
            iced::widget::Space::new().width(Length::Fill),
            text("Clinical Dashboard").size(16).color(color!(0xfdf6e3)),
            iced::widget::Space::new().width(Length::Fill),
        ].align_y(iced::Alignment::Center);

        if self.inference_reconnecting {
            hdr_row = hdr_row.push(text("⟳ Connecting...").size(11).color(color!(0xd4a020)));
            hdr_row = hdr_row.push(iced::widget::Space::new().width(10));
        } else if !self.inference_ok {
            hdr_row = hdr_row.push(text("⚠ Starting inference...").size(11).color(color!(0xe06050)));
            hdr_row = hdr_row.push(iced::widget::Space::new().width(10));
        }

        hdr_row = hdr_row.push(date_nav);
        hdr_row = hdr_row.push(iced::widget::Space::new().width(10));

        let hdr = container(hdr_row)
            .padding(8).width(Length::Fill)
            .style(header_style);

        // Sidebar
        let search = text_input("Search...", &self.search)
            .id(SEARCH_ID)
            .on_input(Msg::Search)
            .on_submit(if self.filtered.len() == 1 {
                Msg::Select(self.filtered[0].id.clone())
            } else {
                Msg::Search(self.search.clone())
            })
            .size(14).padding(6);

        // Clinic clients section (if any in session)
        let mut sidebar_items: Vec<Element<Msg>> = Vec::new();
        let mut list_idx: usize = 0;

        if !self.session.clients.is_empty() {
            for c in &self.session.clients {
                let status_icon = match c.status {
                    ClinicStatus::Done => "✓",
                    ClinicStatus::Pending => "○",
                    ClinicStatus::Dna => "✗",
                    ClinicStatus::Cancelled => "–",
                };
                let status_color = match c.status {
                    ClinicStatus::Done => color!(0x4caf7a),
                    ClinicStatus::Pending => color!(0xfdf6e3),
                    ClinicStatus::Dna => color!(0xe06050),
                    ClinicStatus::Cancelled => color!(0x586e75),
                };
                let time_color = match c.status {
                    ClinicStatus::Cancelled => color!(0x586e75),
                    _ => color!(0x8b8fa4),
                };

                // Build time range string
                let time_range = match (c.time.as_deref(), c.end_time.as_deref()) {
                    (Some(s), Some(e)) => format!("{s}-{e}"),
                    (Some(s), None) => s.to_string(),
                    _ => String::new(),
                };

                // Build the row: icon  time_range  ID  [tag]
                let mut item_row = row![
                    text(status_icon).size(14).color(status_color).width(18),
                ].spacing(6).align_y(iced::Alignment::Center);

                if !time_range.is_empty() {
                    item_row = item_row.push(
                        text(time_range).size(13).color(time_color).width(90)
                    );
                }

                item_row = item_row.push(
                    text(c.id.clone()).size(14).color(status_color).width(50)
                );

                if let Some(ref tag) = c.rate_tag {
                    if !tag.is_empty() {
                        item_row = item_row.push(
                            text(tag.clone()).size(12).color(color!(0x6c71c4))
                        );
                    }
                }

                // Push × dismiss to the right edge
                item_row = item_row.push(iced::widget::Space::new().width(Length::Fill));
                item_row = item_row.push(
                    button(text("×").size(12).color(color!(0x586e75)))
                        .on_press(Msg::RemoveFromClinic(c.id.clone()))
                        .padding([0, 4])
                        .style(button::text)
                );

                let sel = self.selected.as_deref() == Some(&c.id);
                let is_highlighted = self.focus_zone == FocusZone::ClientList
                    && self.highlight == list_idx;

                // Cancelled clients are visible but not selectable
                let b = if c.status == ClinicStatus::Cancelled {
                    button(item_row).width(Length::Fill).padding([3, 6])
                        .style(button::text)
                } else {
                    let b = button(item_row)
                        .on_press(Msg::Select(c.id.clone()))
                        .width(Length::Fill).padding([3, 6]);
                    if sel { b.style(button::primary) } else { b.style(button::text) }
                };

                let item: Element<Msg> = b.into();

                if is_highlighted {
                    sidebar_items.push(
                        container(item).style(highlight_style).into()
                    );
                } else {
                    sidebar_items.push(item);
                }
                list_idx += 1;
            }

        }

        // Search results (shown only when search is active, replaces clinic list)
        if !self.search.is_empty() {
            sidebar_items.clear();
            list_idx = 0;
            for c in &self.filtered {
                let sel = self.selected.as_deref() == Some(&c.id);
                let is_highlighted = self.focus_zone == FocusZone::ClientList
                    && self.highlight == list_idx;

                let status = self.session_client_status(&c.id);
                let label_text = match status {
                    Some(ClinicStatus::Done) => format!("✓ {}", c.id),
                    Some(ClinicStatus::Dna) => format!("✗ {}", c.id),
                    _ => c.id.clone(),
                };
                let b = button(text(label_text).size(12))
                    .on_press(Msg::Select(c.id.clone()))
                    .width(Length::Fill).padding([3, 8]);

                let item: Element<Msg> = if sel {
                    b.style(button::primary).into()
                } else {
                    b.style(button::text).into()
                };

                if is_highlighted {
                    sidebar_items.push(
                        container(item).style(highlight_style).into()
                    );
                } else {
                    sidebar_items.push(item);
                }
                list_idx += 1;
            }
        }

        // Add client input
        let add_input = text_input("+ Add client...", &self.add_client_input)
            .on_input(Msg::AddClientInput)
            .on_submit(Msg::AddClient)
            .size(13).padding(5);

        // Wrap just the scrollable list with a focus ring when ClientList is active
        let client_list_widget = scrollable(Column::with_children(sidebar_items).spacing(1))
            .id(CLIENT_SCROLL_ID)
            .height(Length::Fill);

        let client_list_element: Element<Msg> = if self.focus_zone == FocusZone::ClientList {
            container(client_list_widget).style(focus_ring_style).into()
        } else {
            client_list_widget.into()
        };

        let sidebar_content = column![
            container(text("CLINIC").size(12).color(color!(0x8b8fa4))).padding([6, 8]),
            container(search).padding([4, 6]),
            client_list_element,
            container(add_input).padding([4, 6]),
            if self.all_resolved() && !self.clinic_ended {
                container(
                    button(text("End Clinic").size(13)).on_press(Msg::EndClinic).padding([5, 10]).style(button::success).width(Length::Fill)
                ).padding([4, 6])
            } else {
                container(iced::widget::Space::new().height(0))
            },
        ];

        let sidebar: Element<Msg> = container(sidebar_content)
            .width(250).height(Length::Fill)
            .style(sidebar_style).into();

        // Main content
        let main: Element<Msg> = if let Some(ref id) = self.selected {
            let mut col = column![
                row![
                    text(id).size(16),
                    iced::widget::Space::new().width(Length::Fill),
                    if self.session.clients.iter().any(|c| c.id == *id && c.status == ClinicStatus::Pending) {
                        row![
                            button(text("DNA").size(13)).on_press(Msg::MarkDna(id.clone())).padding([3, 8]).style(button::danger),
                            button(text("Cancel").size(13)).on_press(Msg::MarkCancelled(id.clone())).padding([3, 8]).style(button::secondary),
                        ].spacing(4)
                    } else {
                        row![]
                    },
                ].align_y(iced::Alignment::Center),
                text("Session observation").size(13).color(color!(0x8b8fa4)),
            ].spacing(8);

            // Observation editor — with focus ring when active
            let obs_editor = text_editor(&self.obs)
                .id(OBS_EDITOR_ID)
                .on_action(Msg::Obs)
                .height(150).size(14)
                .font(Font::MONOSPACE);
            if self.focus_zone == FocusZone::ObservationEditor {
                col = col.push(
                    container(obs_editor).style(focus_ring_style)
                );
            } else {
                col = col.push(obs_editor);
            }

            col = col.push(
                row![
                    pick_list(ModelChoice::ALL, Some(&self.model), Msg::Model).text_size(14).padding([4, 8]),
                    if self.busy {
                        button(text("Generating...").size(14)).padding([5, 12])
                    } else if !self.inference_ok {
                        button(text("No inference").size(14)).padding([5, 12])
                    } else {
                        button(text("Generate Note").size(14)).on_press(Msg::Gen).padding([5, 12]).style(button::primary)
                    },
                ].spacing(8).align_y(iced::Alignment::Center),
            );

            if self.show_note {
                col = col.push(rule::horizontal(1));
                col = col.push(row![
                    text("Generated Note").size(15),
                    iced::widget::Space::new().width(Length::Fill),
                    text(&self.status).size(13).color(color!(0x8b8fa4)),
                ]);

                let note_editor = text_editor(&self.note)
                    .id(NOTE_EDITOR_ID)
                    .on_action(Msg::NoteEdit)
                    .height(250).size(14)
                    .font(Font::MONOSPACE);
                if self.focus_zone == FocusZone::NoteEditor {
                    col = col.push(
                        container(note_editor).style(focus_ring_style)
                    );
                } else {
                    col = col.push(note_editor);
                }

                if !self.busy {
                    col = col.push(row![
                        button(text("Accept & Save").size(14)).on_press(Msg::Accept).padding([5, 12]).style(button::success),
                        button(text("Reject").size(14)).on_press(Msg::Reject).padding([5, 12]).style(button::danger),
                        button(text("Compare").size(14)).on_press(Msg::Compare).padding([5, 12]).style(button::secondary),
                    ].spacing(8));
                }
            }

            if !self.status.is_empty() && !self.show_note {
                if self.last_removed.is_some() {
                    col = col.push(row![
                        text(&self.status).size(13).color(color!(0x8b8fa4)),
                        button(text("Undo").size(13)).on_press(Msg::UndoRemove).padding([3, 8]).style(button::secondary),
                    ].spacing(8).align_y(iced::Alignment::Center));
                } else {
                    col = col.push(text(&self.status).size(13).color(color!(0x8b8fa4)));
                }
            }

            if !self.compares.is_empty() {
                col = col.push(rule::horizontal(1));
                col = col.push(row![
                    text("Comparison").size(15),
                    iced::widget::Space::new().width(Length::Fill),
                    button(text("Clear").size(13)).on_press(Msg::ClearCmp).padding([3, 8]).style(button::danger),
                ].align_y(iced::Alignment::Center));
                for (i, (l, t)) in self.compares.iter().enumerate() {
                    col = col.push(column![
                        text(format!("#{i} — {l}")).size(12).color(color!(0x5b9bd5)),
                        text(t).size(13).font(Font::MONOSPACE),
                    ].spacing(2));
                }
            }

            scrollable(container(col).padding(12).width(Length::Fill)).height(Length::Fill).into()
        } else {
            if self.last_removed.is_some() {
                container(
                    row![
                        text(&self.status).size(15).color(color!(0x8b8fa4)),
                        button(text("Undo").size(14)).on_press(Msg::UndoRemove).padding([4, 10]).style(button::secondary),
                    ].spacing(10).align_y(iced::Alignment::Center)
                ).center(Length::Fill).into()
            } else {
                let msg = if self.clinic_ended {
                    &self.status
                } else {
                    "Select a client from the sidebar, or add one to today's clinic."
                };
                container(text(msg).size(15).color(color!(0x8b8fa4)))
                    .center(Length::Fill).into()
            }
        };

        column![hdr, rule::horizontal(1), row![sidebar, main]].height(Length::Fill).into()
    }

    fn subscription(&self) -> Subscription<Msg> {
        Subscription::batch([
            keyboard::listen().map(|event| {
                map_keyboard_event(event).unwrap_or(Msg::NoOp)
            }),
            iced::time::every(std::time::Duration::from_secs(30))
                .map(|_| Msg::InferenceHeartbeat),
        ])
    }
}
