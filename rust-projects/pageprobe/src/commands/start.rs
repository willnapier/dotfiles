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

    // Orphan guard: if a previous pageprobe-Chrome instance for this
    // user-data-dir is still alive (state lost track of its PID, or
    // a prior `stop` failed to clean it up), kill it before launching
    // a new one. Otherwise we end up with two debug-Chromes on
    // (potentially) the same port and the user gets confused about
    // which window is which.
    let orphans = chrome::stop_by_user_data_dir(&user_data_dir).await.unwrap_or(0);
    if orphans > 0 {
        eprintln!("(killed {orphans} orphan Chrome process(es) from a prior session)");
    }

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
