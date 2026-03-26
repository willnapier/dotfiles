use crate::scene::Scene;
use anyhow::Result;
use std::path::Path;

/// Generate a self-contained HTML file that loads Excalidraw from CDN
/// and renders the scene with full interactivity.
pub fn to_html(scene: &Scene) -> Result<String> {
    let scene_json = serde_json::to_string(scene)?;

    Ok(format!(
        r##"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>ea — Excalidraw Viewer</title>
<style>
  html, body, #root {{ margin: 0; padding: 0; width: 100vw; height: 100vh; overflow: hidden; }}
  .loading {{ display: flex; align-items: center; justify-content: center; height: 100vh;
              font-family: system-ui; color: #666; font-size: 18px; }}
  .error {{ color: #c00; font-family: monospace; padding: 2em; white-space: pre-wrap; }}
</style>
</head>
<body>
<div id="root"><div class="loading">Loading Excalidraw…</div></div>

<script>window.__SCENE_DATA__ = {scene_json};</script>

<script type="module">
import {{ createElement }} from 'https://esm.sh/react@18?dev';
import {{ createRoot }} from 'https://esm.sh/react-dom@18/client?dev';
import {{ Excalidraw }} from 'https://esm.sh/@excalidraw/excalidraw@0.18.0?alias=react:react@18,react-dom:react-dom@18';

try {{
  const scene = window.__SCENE_DATA__;
  const App = () => createElement(
    "div",
    {{ style: {{ width: "100vw", height: "100vh" }} }},
    createElement(Excalidraw, {{
      initialData: {{
        elements: scene.elements,
        appState: {{
          viewBackgroundColor: scene.appState.viewBackgroundColor || "#ffffff",
          gridSize: scene.appState.gridSize || null,
        }},
      }},
    }})
  );
  createRoot(document.getElementById("root")).render(createElement(App));
}} catch (e) {{
  document.getElementById("root").innerHTML =
    '<div class="error">Failed to load Excalidraw:\n' + e.message + '</div>';
}}
</script>
</body>
</html>"##
    ))
}

/// Write an HTML viewer file for the scene and return the path.
pub fn write_viewer(scene: &Scene, base_path: &Path) -> Result<std::path::PathBuf> {
    let html = to_html(scene)?;
    let html_path = base_path.with_extension("html");
    std::fs::write(&html_path, html)?;
    Ok(html_path)
}

/// Serve the viewer on localhost and open in browser.
/// Blocks until Ctrl-C or the first request is served (then exits after a grace period).
pub fn serve_and_open(scene: &Scene) -> Result<()> {
    let html = to_html(scene)?;

    // Find an available port
    let server = tiny_http::Server::http("127.0.0.1:0")
        .map_err(|e| anyhow::anyhow!("Failed to start server: {}", e))?;
    let port = server.server_addr().to_ip().map(|a| a.port()).unwrap_or(8080);
    let url = format!("http://127.0.0.1:{}", port);

    eprintln!("Serving at {} (Ctrl-C to stop)", url);

    // Open browser
    #[cfg(target_os = "macos")]
    std::process::Command::new("open").args(["-a", "Safari"]).arg(&url).spawn()?;
    #[cfg(target_os = "linux")]
    std::process::Command::new("xdg-open").arg(&url).spawn()?;

    // Serve requests until Ctrl-C
    let html_bytes = html.into_bytes();
    for request in server.incoming_requests() {
        let response = tiny_http::Response::from_data(html_bytes.clone())
            .with_header(
                tiny_http::Header::from_bytes(
                    &b"Content-Type"[..],
                    &b"text/html; charset=utf-8"[..],
                ).unwrap()
            );
        let _ = request.respond(response);
    }

    Ok(())
}
