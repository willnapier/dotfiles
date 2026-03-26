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
</style>
</head>
<body>
<div id="root"><div class="loading">Loading Excalidraw…</div></div>

<script src="https://unpkg.com/react@18/umd/react.production.min.js"></script>
<script src="https://unpkg.com/react-dom@18/umd/react-dom.production.min.js"></script>
<script src="https://unpkg.com/@excalidraw/excalidraw/dist/excalidraw.production.min.js"></script>

<script>
const sceneData = {scene_json};

const App = () => {{
  return React.createElement(
    "div",
    {{ style: {{ width: "100vw", height: "100vh" }} }},
    React.createElement(ExcalidrawLib.Excalidraw, {{
      initialData: {{
        elements: sceneData.elements,
        appState: {{
          viewBackgroundColor: sceneData.appState.viewBackgroundColor || "#ffffff",
          gridSize: sceneData.appState.gridSize || null,
        }},
      }},
      UIOptions: {{
        canvasActions: {{
          export: {{ saveFileToDisk: true }},
        }},
      }},
    }})
  );
}};

ReactDOM.createRoot(document.getElementById("root")).render(React.createElement(App));
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
