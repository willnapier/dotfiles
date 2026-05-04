use anyhow::Result;

use crate::{cdp, state};

pub async fn run(pattern: String) -> Result<()> {
    let mut s = state::load()?;
    let port = s.port_or_default();

    let (mut browser, handle) = cdp::connect(port).await?;
    let pages = cdp::list_pages(&mut browser).await?;
    let target = cdp::pick_target(&pages, &pattern)?;
    let id = target.target_id.as_ref().to_string();
    let url = target.url.clone();
    let _ = browser.close().await;
    handle.abort();

    s.attached_tab_id = Some(id.clone());
    state::save(&s)?;
    println!("attached: {url}  [{}]", cdp::short_id(&id));
    Ok(())
}
