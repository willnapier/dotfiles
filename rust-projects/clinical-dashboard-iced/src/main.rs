//! Clinical Dashboard — Iced spike
//! Pure Rust, GPU-rendered via wgpu. No WebView, no browser.

use iced::widget::{
    button, column, container, horizontal_rule, pick_list, row, scrollable, text,
    text_editor, text_input, Column,
};
use iced::widget::scrollable::Id as ScrollId;
use iced::keyboard;
use iced::{color, Element, Font, Length, Subscription, Task, Theme};
use std::path::PathBuf;

fn main() -> iced::Result {
    iced::application("Clinical Dashboard", App::update, App::view)
        .subscription(App::subscription)
        .theme(|_| Theme::SolarizedDark)
        .window_size((1100.0, 750.0))
        .window(iced::window::Settings {
            platform_specific: iced::window::settings::PlatformSpecific {
                title_hidden: true,
                titlebar_transparent: true,
                fullsize_content_view: true,
            },
            ..Default::default()
        })
        .run_with(App::new)
}

// ---------------------------------------------------------------------------
// Data
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
        match self { ModelChoice::Q4 => "clinical-voice-q4", ModelChoice::Q8 => "clinical-voice" }
    }
    const ALL: &'static [ModelChoice] = &[ModelChoice::Q4, ModelChoice::Q8];
}

#[derive(Debug, Clone)]
struct ClientEntry { id: String }

fn clients_dir() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or("/tmp".into()))
        .join("Clinical").join("clients")
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

async fn save(id: String, note: String) -> Result<String, String> {
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
    client_scroll_id: ScrollId,
    nav_mode: bool,
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
    Edit,
    Reject,
    Compare,
    ClearCmp,
    KeyUp,
    KeyDown,
    KeyEnter,
}

impl App {
    fn new() -> (Self, Task<Msg>) {
        let clients = load_clients();
        let filtered = clients.clone();
        (Self {
            clients, filtered, search: String::new(), selected: None,
            obs: text_editor::Content::new(), model: ModelChoice::Q4,
            note: text_editor::Content::new(), note_text: String::new(),
            status: String::new(), busy: false,
            show_note: false, compares: Vec::new(), highlight: 0,
            client_scroll_id: ScrollId::unique(), nav_mode: true,
        }, Task::none())
    }

    fn update(&mut self, msg: Msg) -> Task<Msg> {
        match msg {
            Msg::Search(q) => { self.search = q; self.filtered = filter(&self.clients, &self.search); self.highlight = 0; self.nav_mode = true; Task::none() }
            Msg::Select(id) => {
                self.selected = Some(id); self.obs = text_editor::Content::new();
                self.note = text_editor::Content::new(); self.note_text.clear();
                self.show_note = false; self.status.clear(); self.nav_mode = true; Task::none()
            }
            Msg::Obs(a) => { self.obs.perform(a); self.nav_mode = false; Task::none() }
            Msg::NoteEdit(a) => { self.note.perform(a); self.note_text = self.note.text(); self.nav_mode = false; Task::none() }
            Msg::Model(m) => { self.model = m; Task::none() }
            Msg::Gen => {
                let Some(ref id) = self.selected else { return Task::none() };
                let t = self.obs.text(); if t.trim().is_empty() { return Task::none() }
                self.busy = true; self.show_note = true;
                self.note = text_editor::Content::new(); self.note_text.clear();
                self.status = "Generating...".into();
                let id = id.clone(); let m = self.model.model_name().to_string();
                Task::perform(gen(id, t, m), |(n, s)| Msg::GenDone(n, s))
            }
            Msg::GenDone(n, s) => { self.note = text_editor::Content::with_text(&n); self.note_text = n; self.status = format!("Complete — {s:.1}s"); self.busy = false; Task::none() }
            Msg::Accept => {
                let Some(ref id) = self.selected else { return Task::none() };
                let id = id.clone(); let n = self.note_text.clone();
                Task::perform(save(id, n), Msg::Saved)
            }
            Msg::Saved(r) => {
                match r {
                    Ok(_) => { self.status = format!("Saved for {}", self.selected.as_deref().unwrap_or("?")); self.obs = text_editor::Content::new(); self.note = text_editor::Content::new(); self.note_text.clear(); self.show_note = false; }
                    Err(e) => self.status = format!("Failed: {e}"),
                }
                Task::none()
            }
            Msg::Edit => { self.obs = text_editor::Content::with_text(&self.note_text); self.show_note = false; self.status = "Editing".into(); Task::none() }
            Msg::Reject => { self.note = text_editor::Content::new(); self.note_text.clear(); self.show_note = false; self.obs = text_editor::Content::new(); self.status.clear(); Task::none() }
            Msg::Compare => {
                if !self.note_text.is_empty() {
                    let l = format!("{} — {}", self.selected.as_deref().unwrap_or("?"), self.model);
                    self.compares.push((l, self.note_text.clone()));
                }
                Task::none()
            }
            Msg::ClearCmp => { self.compares.clear(); Task::none() }
            Msg::KeyDown => {
                if !self.nav_mode { return Task::none(); }
                if self.highlight + 1 < self.filtered.len() {
                    self.highlight += 1;
                    return self.scroll_to_highlight();
                }
                Task::none()
            }
            Msg::KeyUp => {
                if !self.nav_mode { return Task::none(); }
                if self.highlight > 0 {
                    self.highlight -= 1;
                    return self.scroll_to_highlight();
                }
                Task::none()
            }
            Msg::KeyEnter => {
                if self.highlight < self.filtered.len() {
                    let id = self.filtered[self.highlight].id.clone();
                    self.selected = Some(id); self.obs = text_editor::Content::new();
                    self.note = text_editor::Content::new(); self.note_text.clear();
                    self.show_note = false; self.status.clear();
                    self.search.clear(); self.filtered = filter(&self.clients, &self.search);
                    self.highlight = 0;
                }
                Task::none()
            }
        }
    }

    fn scroll_to_highlight(&self) -> Task<Msg> {
        let total = self.filtered.len().max(1) as f32;
        let ratio = self.highlight as f32 / total;
        scrollable::scroll_to(
            self.client_scroll_id.clone(),
            scrollable::AbsoluteOffset { x: 0.0, y: ratio * total * 25.0 },
        )
    }

    fn view(&self) -> Element<Msg> {
        let today = chrono::Local::now().format("%A %d %B %Y").to_string();

        // Header
        let hdr = container(row![
            iced::widget::Space::with_width(70),
            iced::widget::horizontal_space(),
            text("Clinical Dashboard").size(14).color(color!(0xfdf6e3)),
            iced::widget::horizontal_space(),
            text(today).size(12).color(color!(0x93a1a1)),
            iced::widget::Space::with_width(10),
        ].align_y(iced::Alignment::Center))
            .padding(8)
            .width(Length::Fill)
            .style(|_| container::Style {
                background: Some(iced::Background::Color(color!(0x002b36))),
                ..Default::default()
            });

        // Sidebar
        let search = text_input("Search...", &self.search)
            .on_input(Msg::Search)
            .on_submit(if self.filtered.len() == 1 {
                Msg::Select(self.filtered[0].id.clone())
            } else if self.highlight < self.filtered.len() {
                Msg::KeyEnter
            } else {
                Msg::Search(self.search.clone())
            })
            .size(12).padding(4);

        let btns: Vec<Element<Msg>> = self.filtered.iter().enumerate().map(|(i, c)| {
            let sel = self.selected.as_deref() == Some(&c.id);
            let hl = i == self.highlight;
            let b = button(text(&c.id).size(12))
                .on_press(Msg::Select(c.id.clone()))
                .width(Length::Fill).padding([3, 8]);
            if sel { b.style(button::primary).into() }
            else if hl { b.style(button::secondary).into() }
            else { b.style(button::text).into() }
        }).collect();

        let sidebar = container(column![
            container(text("TODAY").size(10).color(color!(0x8b8fa4))).padding([4, 8]),
            container(search).padding([4, 6]),
            scrollable(Column::with_children(btns).spacing(1))
                .id(self.client_scroll_id.clone())
                .height(Length::Fill),
        ]).width(130).height(Length::Fill);

        // Main
        let main: Element<Msg> = if let Some(ref id) = self.selected {
            let mut col = column![
                text(id).size(14),
                text("Session observation").size(11).color(color!(0x8b8fa4)),
                text_editor(&self.obs).on_action(Msg::Obs).height(150).size(13).font(Font::MONOSPACE),
                row![
                    pick_list(ModelChoice::ALL, Some(&self.model), Msg::Model).text_size(12).padding([3, 6]),
                    if self.busy {
                        button(text("Generating...").size(12)).padding([4, 10])
                    } else {
                        button(text("Generate Note").size(12)).on_press(Msg::Gen).padding([4, 10]).style(button::primary)
                    },
                ].spacing(6).align_y(iced::Alignment::Center),
            ].spacing(6);

            if self.show_note {
                col = col.push(horizontal_rule(1));
                col = col.push(row![
                    text("Generated Note").size(13),
                    iced::widget::horizontal_space(),
                    text(&self.status).size(11).color(color!(0x8b8fa4)),
                ]);
                col = col.push(
                    text_editor(&self.note).on_action(Msg::NoteEdit).height(250).size(12).font(Font::MONOSPACE)
                );
                if !self.busy {
                    col = col.push(row![
                        button(text("Accept & Save").size(12)).on_press(Msg::Accept).padding([4, 10]).style(button::success),
                        button(text("Reject").size(12)).on_press(Msg::Reject).padding([4, 10]).style(button::danger),
                        button(text("Compare").size(12)).on_press(Msg::Compare).padding([4, 10]).style(button::secondary),
                    ].spacing(6));
                }
            }

            if !self.status.is_empty() && !self.show_note {
                col = col.push(text(&self.status).size(11).color(color!(0x8b8fa4)));
            }

            if !self.compares.is_empty() {
                col = col.push(horizontal_rule(1));
                col = col.push(row![
                    text("Comparison").size(13),
                    iced::widget::horizontal_space(),
                    button(text("Clear").size(11)).on_press(Msg::ClearCmp).padding([2, 6]).style(button::danger),
                ].align_y(iced::Alignment::Center));
                for (i, (l, t)) in self.compares.iter().enumerate() {
                    col = col.push(column![
                        text(format!("#{i} — {l}")).size(10).color(color!(0x5b9bd5)),
                        text(t).size(11).font(Font::MONOSPACE),
                    ].spacing(2));
                }
            }

            scrollable(container(col).padding(10).width(Length::Fill)).height(Length::Fill).into()
        } else {
            container(text("Select a client from the sidebar.").size(13).color(color!(0x8b8fa4)))
                .center(Length::Fill).into()
        };

        column![hdr, horizontal_rule(1), row![sidebar, main]].height(Length::Fill).into()
    }

    fn subscription(&self) -> Subscription<Msg> {
        Subscription::none()
    }
}
