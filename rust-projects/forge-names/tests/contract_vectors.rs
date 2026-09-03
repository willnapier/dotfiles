use forge_names::{file_name, find_in_dir, find_in_dir_ci, name_pattern, nfc, nfc_key, rel_key};
use std::path::Path;

const NORMALIZATION: &[(&str, &str, &str)] = &[
    ("Plain ASCII", "Plain ASCII", "plain ascii"),
    ("Zoe\u{0308} Harcombe", "Zoë Harcombe", "zoë harcombe"),
    ("MATÉ", "MATÉ", "maté"),
    ("İ", "İ", "i\u{307}"),
    ("Straße", "Straße", "straße"),
];

#[test]
fn frozen_normalization_vectors() {
    for &(input, expected_nfc, expected_key) in NORMALIZATION {
        assert_eq!(nfc(input), expected_nfc, "NFC vector for {input:?}");
        assert_eq!(nfc_key(input), expected_key, "key vector for {input:?}");
    }
    assert_eq!(rel_key("Zoe\u{0308}\\MATÉ"), "zoë/maté");
}

#[test]
fn lookup_vectors_return_the_walked_spelling() {
    const NFD: &str = "Zoe\u{0308} Harcombe";
    const NFC: &str = "Zoë Harcombe";
    let directory = tempfile::tempdir().unwrap();
    let on_disk = directory.path().join(format!("{NFD}.md"));
    std::fs::write(&on_disk, b"fixture").unwrap();

    assert_eq!(
        find_in_dir(directory.path(), &format!("{NFC}.md")),
        Some(on_disk.clone())
    );
    assert_eq!(
        find_in_dir_ci(directory.path(), "ZOË HARCOMBE.MD"),
        Some(on_disk.clone())
    );
    assert_eq!(file_name(&on_disk), format!("{NFC}.md"));
    assert_ne!(
        on_disk.file_name().unwrap(),
        Path::new(&format!("{NFC}.md")).file_name().unwrap()
    );
}

#[test]
fn text_pattern_vectors_match_both_unicode_spellings() {
    const NFD: &str = "Zoe\u{0308} Harcombe";
    const NFC: &str = "Zoë Harcombe";
    let expression = regex::Regex::new(&format!(r"^{}$", name_pattern(NFC))).unwrap();
    assert!(expression.is_match(NFC));
    assert!(expression.is_match(NFD));
    assert!(!expression.is_match("Zoe Harcombe"));
}
