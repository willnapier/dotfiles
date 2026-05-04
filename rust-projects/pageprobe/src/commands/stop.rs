use anyhow::Result;
use std::time::Duration;

use crate::{chrome, state};

pub async fn run() -> Result<()> {
    let mut s = state::load()?;

    let mut killed = 0usize;

    if let Some(pid) = s.chrome_pid {
        chrome::stop(pid, Duration::from_secs(3)).await?;
        killed += 1;
    }

    // Defensive cleanup: any leftover Chrome bound to the same user-data-dir.
    if let Some(dir) = s.user_data_dir.clone() {
        let extra = chrome::stop_by_user_data_dir(&dir).await?;
        killed += extra;
    }

    s.chrome_pid = None;
    s.port = None;
    s.attached_tab_id = None;
    // Keep user_data_dir so the next `start` re-uses the same profile if
    // the user invokes `start` without flags.
    state::save(&s)?;

    if killed == 0 {
        println!("no debug-Chrome was running.");
    } else {
        println!("stopped {killed} Chrome process(es).");
    }
    Ok(())
}
