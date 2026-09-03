use std::path::{Path, PathBuf};

/// Root directory for all card files: ~/Forge/sr/{deck}/{id}.md
pub fn sr_dir() -> PathBuf {
    let home = dirs::home_dir().expect("cannot determine home directory");
    home.join("Forge").join("sr")
}

/// The path→name boundary for deck names (forge-names rule): a deck name is
/// NFC from the moment it enters — CLI argument or card frontmatter — so one
/// deck is one directory and one frontmatter value on every host.
pub fn deck_key(deck: &str) -> String {
    forge_names::nfc(deck)
}

/// Directory for a specific deck. An existing directory is found by NFC
/// comparison and its own path returned (an NFD-spelled directory synced from
/// the Mac is reused, not twinned); only a new deck is joined, NFC.
pub fn deck_dir(deck: &str) -> PathBuf {
    deck_dir_in(&sr_dir(), deck)
}

pub fn deck_dir_in(root: &Path, deck: &str) -> PathBuf {
    forge_names::find_in_dir(root, deck).unwrap_or_else(|| root.join(deck_key(deck)))
}

/// Path for a specific card file
pub fn card_path(deck: &str, id: &str) -> PathBuf {
    deck_dir(deck).join(format!("{}.md", id))
}

#[cfg(test)]
mod tests {
    use super::*;
    const NFD: &str = "Zoe\u{0308}";
    const NFC: &str = "Zoë";

    #[test]
    fn nfc_and_nfd_deck_names_map_to_one_directory() {
        let root = tempfile::tempdir().unwrap();
        // a new deck is joined NFC whichever spelling arrived
        assert_eq!(deck_dir_in(root.path(), NFD).file_name().unwrap().to_str().unwrap(), NFC);
        assert_eq!(deck_dir_in(root.path(), NFC).file_name().unwrap().to_str().unwrap(), NFC);
        // an NFD directory synced from the Mac is reused for both spellings
        let synced = root.path().join(NFD);
        std::fs::create_dir(&synced).unwrap();
        let via_nfc = deck_dir_in(root.path(), NFC);
        let via_nfd = deck_dir_in(root.path(), NFD);
        assert_eq!(via_nfc, via_nfd);
        assert_eq!(forge_names::file_name(&via_nfc), NFC);
        std::fs::create_dir_all(&via_nfc).unwrap();
        std::fs::create_dir_all(&via_nfd).unwrap();
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1, "one deck directory, not twins");
    }
}
