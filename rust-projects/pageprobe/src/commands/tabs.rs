use anyhow::Result;
use serde::Serialize;

use crate::{cdp, state};

#[derive(Serialize)]
struct TabRow<'a> {
    target_id: &'a str,
    short_id: String,
    url: &'a str,
    title: &'a str,
}

pub async fn run(json: bool) -> Result<()> {
    let s = state::load()?;
    let port = s.port_or_default();

    let (mut browser, handle) = cdp::connect(port).await?;
    let pages = cdp::list_pages(&mut browser).await?;

    if json {
        let rows: Vec<TabRow> = pages
            .iter()
            .map(|t| TabRow {
                target_id: t.target_id.as_ref(),
                short_id: cdp::short_id(t.target_id.as_ref()),
                url: &t.url,
                title: &t.title,
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&rows)?);
    } else if pages.is_empty() {
        println!("(no tabs open)");
    } else {
        for t in &pages {
            let id = cdp::short_id(t.target_id.as_ref());
            let url = cdp::truncate(&t.url, 60);
            let title = cdp::truncate(&t.title, 40);
            println!("{id}  {url:<60}  {title}");
        }
    }

    // Drop closes the WebSocket; we deliberately do NOT call
    // `browser.close()` (that sends `Browser.close` and shuts Chrome down).
    handle.abort();
    drop(browser);
    Ok(())
}
