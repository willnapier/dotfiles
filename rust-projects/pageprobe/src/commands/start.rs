use anyhow::Result;
use std::path::PathBuf;

use crate::{chrome, state};

pub async fn run(port: u16, user_data_dir: Option<PathBuf>) -> Result<()> {
    let mut s = state::load()?;
    if let Some(p) = s.chrome_pid
        && chrome::debug_chrome_alive(s.port_or_default()).await
    {
        anyhow::bail!(
            "pageprobe already running (PID {p}, port {}). Use `pageprobe stop` first.",
            s.port_or_default()
        );
    }

    let user_data_dir = match user_data_dir {
        Some(p) => p,
        None => state::default_user_data_dir()?,
    };

    let pid = chrome::launch(port, &user_data_dir).await?;

    s.chrome_pid = if pid == 0 { None } else { Some(pid) };
    s.port = Some(port);
    s.user_data_dir = Some(user_data_dir.clone());
    s.attached_tab_id = None;
    state::save(&s)?;

    if pid == 0 {
        println!(
            "Chrome launched on port {port} (PID could not be resolved; \
             stop will fall back to pgrep). user-data-dir: {}",
            user_data_dir.display()
        );
    } else {
        println!(
            "Chrome launched (PID {pid}, port {port}). user-data-dir: {}",
            user_data_dir.display()
        );
    }
    Ok(())
}
