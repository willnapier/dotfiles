use std::process::{Child, Command};
use std::sync::Mutex;
use tauri::Manager;

const PORT: u16 = 3456;

struct ServerProcess(Mutex<Option<Child>>);

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            // Spawn `clinical-product dashboard` as a subprocess
            let child = Command::new("clinical-product")
                .arg("dashboard")
                .arg("--port")
                .arg(PORT.to_string())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .expect("Failed to start clinical-product dashboard. Is it installed?");

            app.manage(ServerProcess(Mutex::new(Some(child))));

            // Wait for the server to be ready
            let start = std::time::Instant::now();
            loop {
                if start.elapsed() > std::time::Duration::from_secs(5) {
                    eprintln!("Warning: server may not be ready yet");
                    break;
                }
                if std::net::TcpStream::connect(format!("127.0.0.1:{PORT}")).is_ok() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }

            // Create the main window pointing at the local server
            let url = url::Url::parse(&format!("http://localhost:{PORT}")).unwrap();
            tauri::WebviewWindowBuilder::new(
                app,
                "main",
                tauri::WebviewUrl::External(url),
            )
            .title("Clinical Dashboard")
            .inner_size(1100.0, 750.0)
            .min_inner_size(600.0, 400.0)
            .build()?;

            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event {
                // Kill the server subprocess on exit
                if let Some(state) = app.try_state::<ServerProcess>() {
                    if let Ok(mut guard) = state.0.lock() {
                        if let Some(mut child) = guard.take() {
                            let _ = child.kill();
                        }
                    }
                }
            }
        });
}
