//! Clinical Dashboard — Dioxus spike
//! Pure Rust UI. Single binary, native window.

use dioxus::prelude::*;
use std::path::PathBuf;

fn main() {
    dioxus::launch(app);
}

// ---------------------------------------------------------------------------
// Data
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
struct ClientEntry {
    id: String,
}

fn clients_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    PathBuf::from(home).join("Clinical").join("clients")
}

fn list_clients() -> Vec<ClientEntry> {
    let dir = clients_dir();
    let mut clients = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                if let Some(name) = entry.file_name().to_str() {
                    if !name.starts_with('.') {
                        clients.push(ClientEntry { id: name.to_string() });
                    }
                }
            }
        }
    }
    clients.sort_by(|a, b| a.id.cmp(&b.id));
    clients
}

async fn run_generate(client_id: String, observation: String, model: String) -> String {
    let mut cmd = tokio::process::Command::new("clinical");
    cmd.arg("note").arg(&client_id).arg(&observation)
        .arg("--no-save").arg("--yes");
    if !model.is_empty() {
        cmd.arg("--model-override").arg(&model);
    }
    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    match cmd.spawn() {
        Ok(child) => match child.wait_with_output().await {
            Ok(out) => String::from_utf8_lossy(&out.stdout).to_string(),
            Err(e) => format!("[error: {e}]"),
        },
        Err(e) => format!("[error: {e}]"),
    }
}

async fn run_save(client_id: String, note: String) -> Result<String, String> {
    let mut child = tokio::process::Command::new("clinical")
        .arg("note-save").arg(&client_id)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn().map_err(|e| e.to_string())?;
    if let Some(mut stdin) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        stdin.write_all(note.as_bytes()).await.map_err(|e| e.to_string())?;
    }
    let out = child.wait_with_output().await.map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).to_string())
    }
}

// ---------------------------------------------------------------------------
// CSS
// ---------------------------------------------------------------------------

const CSS: &str = r#"
* { margin:0; padding:0; box-sizing:border-box; }
:root {
    --bg:#0f1117; --card:#1a1d27; --sidebar:#14161e; --border:#2a2d3a;
    --text:#e2e4e9; --muted:#8b8fa4; --accent:#5b9bd5; --success:#4caf7a;
    --danger:#e06050; --warn:#d4a020; --cmp:#6b5b95;
    --font:-apple-system,BlinkMacSystemFont,"Segoe UI",Roboto,sans-serif;
    --mono:"SF Mono","Cascadia Code","Fira Code",monospace;
}
body { font:14px var(--font); background:var(--bg); color:var(--text);
       height:100vh; overflow:hidden; display:flex; flex-direction:column; }
.hdr { display:flex; justify-content:space-between; align-items:center;
       padding:6px 12px; border-bottom:1px solid var(--border); background:var(--card); }
.hdr h1 { font-size:14px; } .hdr .dt { font-size:12px; color:var(--muted); }
.lay { display:flex; flex:1; min-height:0; }
.sb { width:140px; min-width:140px; background:var(--sidebar);
      border-right:1px solid var(--border); display:flex; flex-direction:column; }
.sb-h { padding:4px 8px; border-bottom:1px solid var(--border);
        font-size:11px; font-weight:600; text-transform:uppercase; color:var(--muted); }
.sb-s { padding:4px 6px; border-bottom:1px solid var(--border); }
.sb-s input { width:100%; padding:4px 6px; border:1px solid var(--border);
              border-radius:4px; font-size:12px; background:var(--bg); color:var(--text); outline:none; }
.sb-s input:focus { border-color:var(--accent); }
.cl { list-style:none; overflow-y:auto; flex:1; padding:2px 0; }
.cl li { padding:4px 8px; cursor:pointer; font-size:13px; border-left:3px solid transparent; }
.cl li:hover { background:#1f2233; }
.cl li.a { background:#1f2233; border-left-color:var(--accent); font-weight:500; }
.cl li.hl { background:#1a2235; border-left-color:var(--muted); }
.mn { flex:1; overflow-y:auto; padding:10px 14px; display:flex;
      flex-direction:column; align-items:center; gap:8px; }
.cd { background:var(--card); border:1px solid var(--border);
      border-radius:6px; padding:10px 12px; width:100%; max-width:800px; }
.cd h2 { font-size:14px; font-weight:600; margin-bottom:4px; }
label { font-size:12px; font-weight:500; color:var(--muted); display:block; margin-bottom:3px; }
textarea { width:100%; padding:8px; border:1px solid var(--border); border-radius:6px;
           font:13px/1.5 var(--mono); resize:none; background:var(--bg); color:var(--text); }
textarea:focus { outline:none; border-color:var(--accent); }
.act { display:flex; gap:6px; margin-top:8px; align-items:center; }
.b { padding:5px 12px; border:none; border-radius:4px; font-size:13px;
     font-weight:500; cursor:pointer; color:#fff; }
.b:disabled { opacity:0.5; cursor:not-allowed; }
.bp { background:var(--accent); } .ba { background:var(--success); }
.be { background:var(--warn); } .br { background:var(--danger); } .bc { background:var(--cmp); }
select { padding:5px 6px; border:1px solid var(--border); border-radius:4px;
         font-size:13px; background:var(--bg); color:var(--text); }
.no { background:#12141c; border:1px solid var(--border); border-radius:6px;
      padding:10px; font:13px/1.6 var(--mono); white-space:pre-wrap;
      word-wrap:break-word; min-height:100px; max-height:50vh; overflow-y:auto; }
.st { font-size:12px; color:var(--muted); }
.emp { display:flex; align-items:center; justify-content:center; min-height:150px; color:var(--muted); }
.tst { position:fixed; bottom:16px; left:50%; transform:translateX(-50%);
       background:var(--text); color:var(--bg); padding:6px 16px; border-radius:6px; font-size:13px; z-index:100; }
.ce { background:#12141c; border:1px solid var(--border); border-radius:4px; padding:8px; margin-top:6px; }
.ce .cl2 { font-size:11px; font-weight:600; color:var(--accent); margin-bottom:4px; }
.ce pre { font:12px/1.5 var(--mono); white-space:pre-wrap; word-wrap:break-word; margin:0; }
::-webkit-scrollbar { width:6px; }
::-webkit-scrollbar-track { background:transparent; }
::-webkit-scrollbar-thumb { background:#3a3d4a; border-radius:3px; }
"#;

const INIT_JS: &str = r#"
// No-op — arrow key handling is done in Dioxus
"#;

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

fn app() -> Element {
    let clients = use_signal(|| list_clients());
    let mut selected = use_signal(|| None::<String>);
    let mut search = use_signal(|| String::new());
    let mut highlight_idx = use_signal(|| 0usize);
    let mut obs = use_signal(|| String::new());
    let mut note = use_signal(|| String::new());
    let mut generating = use_signal(|| false);
    let mut status = use_signal(|| String::new());
    let mut show_note = use_signal(|| false);
    let mut show_actions = use_signal(|| false);
    let mut toast = use_signal(|| None::<String>);
    let mut model = use_signal(|| "clinical-voice-q4".to_string());
    let mut editing = use_signal(|| false);
    let mut compares = use_signal(|| Vec::<(String, String)>::new());

    let filtered: Vec<ClientEntry> = clients.read().iter()
        .filter(|c| { let q = search.read().to_uppercase(); q.is_empty() || c.id.to_uppercase().contains(&q) })
        .cloned().collect();

    let today = {
        let now = std::time::SystemTime::now();
        let secs = now.duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        // Simple date — good enough for spike
        format!("{}", chrono::Local::now().format("%A %d %B %Y"))
    };

    // Inject JS to suppress arrow-key scrolling on inputs
    use_effect(|| { document::eval(INIT_JS); });

    rsx! {
        style { {CSS} }
        div { class: "hdr", h1 { "Clinical Dashboard" } span { class: "dt", "{today}" } }
        div { class: "lay",
            onkeydown: move |e| {
                // Global arrow key navigation for client list (unless in textarea)
                let q = search.read().to_uppercase();
                let filt: Vec<_> = clients.read().iter()
                    .filter(|c| q.is_empty() || c.id.to_uppercase().contains(&q))
                    .cloned().collect();
                match e.key() {
                    Key::ArrowDown => {
                        e.prevent_default();
                        let idx = *highlight_idx.read();
                        if idx + 1 < filt.len() { highlight_idx.set(idx + 1); }
                    }
                    Key::ArrowUp => {
                        e.prevent_default();
                        let idx = *highlight_idx.read();
                        if idx > 0 { highlight_idx.set(idx - 1); }
                    }
                    Key::Enter => {
                        let idx = *highlight_idx.read();
                        if idx < filt.len() && selected.read().is_none() {
                            selected.set(Some(filt[idx].id.clone()));
                            search.set(String::new());
                            highlight_idx.set(0);
                            obs.set(String::new()); note.set(String::new());
                            show_note.set(false); show_actions.set(false); editing.set(false);
                        }
                    }
                    _ => {}
                }
            },
            div { class: "sb",
                div { class: "sb-h", "Today" }
                div { class: "sb-s",
                    input {
                        r#type: "text", placeholder: "Search...",
                        value: "{search}",
                        oninput: move |e| { search.set(e.value()); highlight_idx.set(0); },
                        onkeydown: move |e| {
                            let q = search.read().to_uppercase();
                            let filt: Vec<_> = clients.read().iter()
                                .filter(|c| q.is_empty() || c.id.to_uppercase().contains(&q))
                                .cloned().collect();
                            match e.key() {
                                Key::ArrowDown => {
                                    e.prevent_default();
                                    e.stop_propagation();
                                    let idx = *highlight_idx.read();
                                    if idx + 1 < filt.len() { highlight_idx.set(idx + 1); }
                                }
                                Key::ArrowUp => {
                                    e.prevent_default();
                                    e.stop_propagation();
                                    let idx = *highlight_idx.read();
                                    if idx > 0 { highlight_idx.set(idx - 1); }
                                }
                                Key::Enter => {
                                    e.prevent_default();
                                    let idx = *highlight_idx.read();
                                    // If one match or arrow-selected
                                    let target = if filt.len() == 1 {
                                        Some(filt[0].id.clone())
                                    } else if idx < filt.len() {
                                        Some(filt[idx].id.clone())
                                    } else {
                                        None
                                    };
                                    if let Some(id) = target {
                                        selected.set(Some(id));
                                        search.set(String::new());
                                        highlight_idx.set(0);
                                        obs.set(String::new()); note.set(String::new());
                                        show_note.set(false); show_actions.set(false); editing.set(false);
                                    }
                                }
                                _ => {}
                            }
                        },
                    }
                }
                ul { class: "cl",
                    for (i, c) in filtered.iter().enumerate() {
                        li {
                            class: {
                                let is_selected = selected.read().as_deref() == Some(&c.id);
                                let is_highlighted = i == *highlight_idx.read();
                                match (is_selected, is_highlighted) {
                                    (true, _) => "a",
                                    (false, true) => "hl",
                                    _ => "",
                                }
                            },
                            onclick: { let id = c.id.clone(); move |_| {
                                selected.set(Some(id.clone()));
                                obs.set(String::new()); note.set(String::new());
                                show_note.set(false); show_actions.set(false); editing.set(false);
                            }},
                            "{c.id}"
                        }
                    }
                }
            }
            div { class: "mn",
                if let Some(ref id) = *selected.read() {
                    div { class: "cd", h2 { "{id}" } }
                    div { class: "cd",
                        label { "Session observation" }
                        textarea { rows: "8", placeholder: "Client presented with...",
                            value: "{obs}", oninput: move |e| obs.set(e.value()) }
                        div { class: "act",
                            select { value: "{model}", onchange: move |e| model.set(e.value()),
                                option { value: "clinical-voice-q4", "Q4" }
                                option { value: "clinical-voice-q8", "Q8" }
                            }
                            button { class: "b bp",
                                disabled: obs.read().trim().is_empty() || *generating.read(),
                                onclick: { let id = id.clone(); move |_| {
                                    let id = id.clone(); let o = obs.read().clone(); let m = model.read().clone();
                                    generating.set(true); show_note.set(true); show_actions.set(false);
                                    note.set(String::new()); status.set("Generating...".into()); editing.set(false);
                                    spawn(async move {
                                        let t = std::time::Instant::now();
                                        let result = run_generate(id, o, m).await;
                                        let secs = t.elapsed().as_secs_f64();
                                        note.set(result); status.set(format!("Complete — {secs:.1}s"));
                                        show_actions.set(true); generating.set(false);
                                    });
                                }},
                                if *generating.read() { "Generating..." } else { "Generate Note" }
                            }
                        }
                    }
                    if *show_note.read() {
                        div { class: "cd",
                            div { style: "display:flex;justify-content:space-between;margin-bottom:6px;",
                                h2 { "Generated Note" } span { class: "st", "{status}" }
                            }
                            if *editing.read() {
                                textarea { rows: "20", value: "{note}",
                                    oninput: move |e| note.set(e.value()) }
                                div { class: "act",
                                    button { class: "b ba", onclick: move |_| editing.set(false), "Done Editing" }
                                }
                            } else {
                                div { class: "no", "{note}" }
                            }
                            if *show_actions.read() && !*editing.read() {
                                div { class: "act",
                                    button { class: "b ba", onclick: { let id = id.clone(); move |_| {
                                        let id = id.clone(); let n = note.read().clone();
                                        spawn(async move {
                                            match run_save(id.clone(), n).await {
                                                Ok(_) => { toast.set(Some(format!("Saved for {id}"))); obs.set(String::new());
                                                    note.set(String::new()); show_note.set(false); show_actions.set(false); }
                                                Err(e) => toast.set(Some(format!("Failed: {e}"))),
                                            }
                                        });
                                    }}, "Accept & Save" }
                                    button { class: "b be", onclick: move |_| editing.set(true), "Edit" }
                                    button { class: "b br", onclick: move |_| {
                                        note.set(String::new()); show_note.set(false);
                                        show_actions.set(false); obs.set(String::new());
                                    }, "Reject" }
                                    button { class: "b bc", onclick: { let id = id.clone(); move |_| {
                                        let n = note.read().clone(); let m = model.read().clone();
                                        let lbl = format!("{} — {}", id, if m.contains("q4") {"Q4"} else {"Q8"});
                                        compares.write().push((lbl, n));
                                    }}, "Compare" }
                                }
                            }
                        }
                    }
                    if !compares.read().is_empty() {
                        div { class: "cd",
                            div { style: "display:flex;justify-content:space-between;margin-bottom:6px;",
                                h2 { "Comparison" }
                                button { class: "b br", onclick: move |_| compares.write().clear(), "Clear" }
                            }
                            for (i, (lbl, txt)) in compares.read().iter().enumerate() {
                                div { class: "ce",
                                    div { class: "cl2", "#{i} — {lbl}" }
                                    pre { "{txt}" }
                                }
                            }
                        }
                    }
                } else {
                    div { class: "cd emp", "Select a client from the sidebar to begin." }
                }
            }
        }
        if let Some(ref msg) = *toast.read() {
            div { class: "tst", onclick: move |_| toast.set(None), "{msg}" }
        }
    }
}
