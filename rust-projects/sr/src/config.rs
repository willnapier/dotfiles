use std::path::PathBuf;

/// Root directory for all card files: ~/Forge/sr/{deck}/{id}.md
pub fn sr_dir() -> PathBuf {
    let home = dirs::home_dir().expect("cannot determine home directory");
    home.join("Forge").join("sr")
}

/// Directory for a specific deck
pub fn deck_dir(deck: &str) -> PathBuf {
    sr_dir().join(deck)
}

/// Path for a specific card file
pub fn card_path(deck: &str, id: &str) -> PathBuf {
    deck_dir(deck).join(format!("{}.md", id))
}
