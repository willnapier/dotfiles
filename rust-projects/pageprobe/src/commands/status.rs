use anyhow::Result;

use crate::{cdp, chrome, state};

pub async fn run(json: bool) -> Result<()> {
    let s = state::load()?;
    let port = s.port_or_default();
    let alive = chrome::debug_chrome_alive(port).await;

    let mut tab_count = 0usize;
    let mut attached_url: Option<String> = None;

    if alive
        && let Ok((mut browser, handle)) = cdp::connect(port).await
    {
        if let Ok(pages) = cdp::list_pages(&mut browser).await {
            tab_count = pages.len();
            if let Some(id) = &s.attached_tab_id
                && let Some(t) = pages.iter().find(|t| t.target_id.as_ref() == id)
            {
                attached_url = Some(t.url.clone());
            }
        }
        // Best-effort close.
        let _ = browser.close().await;
        handle.abort();
    }

    if json {
        let body = serde_json::json!({
            "running": alive,
            "pid": s.chrome_pid,
            "port": s.port,
            "attached_tab_id": s.attached_tab_id,
            "attached_tab_url": attached_url,
            "total_tabs": tab_count,
            "user_data_dir": s.user_data_dir,
        });
        println!("{}", serde_json::to_string_pretty(&body)?);
        return Ok(());
    }

    if alive {
        match s.chrome_pid {
            Some(pid) => println!("running: yes (PID {pid}, port {port})"),
            None => println!("running: yes (port {port}; PID unknown)"),
        }
    } else {
        println!("running: no");
        return Ok(());
    }

    match (&s.attached_tab_id, &attached_url) {
        (Some(id), Some(url)) => println!("attached tab: {url}  [{}]", cdp::short_id(id)),
        (Some(id), None) => println!("attached tab: <not in current tab list> [{}]", cdp::short_id(id)),
        (None, _) => println!("attached tab: (none — run `pageprobe attach <pattern>`)"),
    }
    println!("total open tabs: {tab_count}");
    Ok(())
}
