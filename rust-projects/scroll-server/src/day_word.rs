//! Day-word computation per §3 of the spec.
//!
//! `day_word = WORD_LIST[ HMAC_SHA256(seed, date_str) mod len(WORD_LIST) ]`
//! where `date_str = YYYY-MM-DD in Europe/London, day boundary at 03:00 local`.

use chrono::{DateTime, Datelike, Duration, Utc};
use chrono_tz::Europe::London;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use subtle::ConstantTimeEq;

type HmacSha256 = Hmac<Sha256>;

/// Compute the `YYYY-MM-DD` date string for the given UTC instant,
/// interpreted in Europe/London with the day boundary shifted to 03:00 local.
///
/// I.e. 02:59 local reads as "yesterday"; 03:00 local reads as "today".
pub fn date_str_for(now_utc: DateTime<Utc>) -> String {
    let local = now_utc.with_timezone(&London);
    // Shift by -3h so the 03:00 boundary lands on midnight in the shifted frame;
    // then take the calendar date.
    let shifted = local - Duration::hours(3);
    format!("{:04}-{:02}-{:02}", shifted.year(), shifted.month(), shifted.day())
}

/// Compute today's day-word given the seed bytes, the word list, and the current UTC time.
///
/// Returns the uppercase word from `word_list` selected by
/// `HMAC_SHA256(seed, date_str)` truncated to 4 bytes BE mod `word_list.len()`.
pub fn day_word(seed: &[u8], word_list: &[String], now_utc: DateTime<Utc>) -> String {
    let date = date_str_for(now_utc);
    let mut mac = HmacSha256::new_from_slice(seed).expect("HMAC key of any size is valid");
    mac.update(date.as_bytes());
    let tag = mac.finalize().into_bytes();
    let truncated = u32::from_be_bytes([tag[0], tag[1], tag[2], tag[3]]);
    let idx = (truncated as usize) % word_list.len();
    word_list[idx].clone()
}

/// Constant-time comparison of two words.
pub fn words_match(a: &str, b: &str) -> bool {
    a.as_bytes().ct_eq(b.as_bytes()).into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn fake_list() -> Vec<String> {
        vec![
            "HARBOUR", "MEADOW", "COMPASS", "LANTERN", "ORCHARD",
            "COTTAGE", "PEBBLE", "RIBBON", "THUNDER", "WILLOW",
            "COPPER", "ANCHOR", "GARDEN", "MOUNTAIN", "SAPPHIRE",
            "OXFORD", "MARBLE", "CANYON", "FORTRESS", "OCEAN",
            "PARCHMENT", "HORIZON", "COMET", "GRANITE", "JOURNEY",
            "MELODY", "PRAIRIE", "RIVER", "SIGNAL", "VELVET",
        ]
        .into_iter()
        .map(|s| s.to_string())
        .collect()
    }

    #[test]
    fn determinism_same_seed_same_date_same_word() {
        let seed = b"test-seed-32-bytes-aaaaaaaaaaaaa";
        let list = fake_list();
        let t = Utc.with_ymd_and_hms(2026, 5, 16, 12, 0, 0).unwrap();
        let w1 = day_word(seed, &list, t);
        let w2 = day_word(seed, &list, t);
        assert_eq!(w1, w2);
        assert!(list.contains(&w1));
    }

    #[test]
    fn different_seeds_produce_different_words() {
        // Pick a date where two arbitrary seeds happen to differ.
        // With a 30-word list, collision probability is 1/30; trying a few
        // seeds guarantees we find a pair that differ.
        let list = fake_list();
        let t = Utc.with_ymd_and_hms(2026, 5, 16, 12, 0, 0).unwrap();
        let w_a = day_word(b"seed-alpha", &list, t);
        let mut found_diff = false;
        for s in [b"seed-beta" as &[u8], b"seed-gamma", b"seed-delta", b"seed-epsilon"] {
            if day_word(s, &list, t) != w_a {
                found_diff = true;
                break;
            }
        }
        assert!(found_diff, "expected some seed to produce a different word");
    }

    #[test]
    fn rotation_boundary_02_59_vs_03_00_london() {
        // 02:59 Europe/London on 2026-05-16 reads as the "2026-05-15" word.
        // 03:00 Europe/London on 2026-05-16 reads as the "2026-05-16" word.
        let before = London
            .with_ymd_and_hms(2026, 5, 16, 2, 59, 0)
            .unwrap()
            .with_timezone(&Utc);
        let after = London
            .with_ymd_and_hms(2026, 5, 16, 3, 0, 0)
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(date_str_for(before), "2026-05-15");
        assert_eq!(date_str_for(after), "2026-05-16");

        // And the actual word should differ across the boundary for nearly
        // any seed (1/30 collision risk — try a few if first matches).
        let list = fake_list();
        let mut found_diff = false;
        for s in [
            b"seed-1" as &[u8], b"seed-2", b"seed-3", b"seed-4", b"seed-5",
        ] {
            if day_word(s, &list, before) != day_word(s, &list, after) {
                found_diff = true;
                break;
            }
        }
        assert!(found_diff, "expected the boundary to flip the word for some seed");
    }

    #[test]
    fn date_str_uses_london_not_utc() {
        // 2026-05-16 02:30 UTC = 2026-05-16 03:30 BST (London is UTC+1 in May).
        // 03:30 local is past the 03:00 boundary, so date_str = "2026-05-16".
        let t = Utc.with_ymd_and_hms(2026, 5, 16, 2, 30, 0).unwrap();
        assert_eq!(date_str_for(t), "2026-05-16");
    }

    #[test]
    fn words_match_constant_time() {
        assert!(words_match("HARBOUR", "HARBOUR"));
        assert!(!words_match("HARBOUR", "MEADOW"));
        assert!(!words_match("HARBOUR", "HARBOURX"));
    }
}
