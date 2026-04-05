//! FSRS-4 spaced repetition scheduler.
//!
//! References:
//!   - FSRS paper: https://github.com/open-spaced-repetition/fsrs4anki/wiki/The-Algorithm
//!   - Default weights from the FSRS-4 parameter set
//!
//! Key insight: with 90% desired retention, next_interval ≈ stability (days).
//! This is a mathematical consequence of FSRS's power forgetting curve.

/// Review ratings (match Anki/FSRS convention)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rating {
    Again = 1,
    Hard = 2,
    Good = 3,
    Easy = 4,
}

impl Rating {
    pub fn from_u8(n: u8) -> Option<Self> {
        match n {
            1 => Some(Rating::Again),
            2 => Some(Rating::Hard),
            3 => Some(Rating::Good),
            4 => Some(Rating::Easy),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Rating::Again => "Again",
            Rating::Hard => "Hard",
            Rating::Good => "Good",
            Rating::Easy => "Easy",
        }
    }
}

impl std::fmt::Display for Rating {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// FSRS-4 default weights (from the open-spaced-repetition project)
const W: [f64; 17] = [
    0.4072,  // w0: initial stability for Again
    1.1829,  // w1: initial stability for Hard
    3.1262,  // w2: initial stability for Good
    15.4722, // w3: initial stability for Easy
    7.2102,  // w4: initial difficulty base
    0.5316,  // w5: difficulty scale factor
    1.0651,  // w6: difficulty change per rating step
    0.0589,  // w7: difficulty mean-reversion weight
    1.4824,  // w8: stability-after-review exponent
    0.1483,  // w9: stability decay for review
    1.0150,  // w10: retrievability bonus factor
    1.9319,  // w11: stability after forgetting base
    0.1100,  // w12: stability after forgetting difficulty exponent
    0.2900,  // w13: stability after forgetting stability exponent
    2.2700,  // w14: stability after forgetting retrievability exponent
    0.2500,  // w15: Easy multiplier
    2.9898,  // w16: Hard multiplier (< 1, applied as W[16]^-1 below)
];

/// Desired retention rate (90%)
const DESIRED_RETENTION: f64 = 0.9;

/// Power forgetting curve exponent
const DECAY: f64 = -0.5;

/// Forgetting curve factor: = (1/desired_retention)^(1/DECAY) - 1
/// With retention=0.9 and DECAY=-0.5: FACTOR = 0.9^(-2) - 1 = 1/0.81 - 1 ≈ 0.2346
const FACTOR: f64 = 19.0 / 81.0;

/// Result of scheduling a card after a review.
#[derive(Debug, Clone)]
pub struct ScheduleResult {
    /// New stability value (days)
    pub stability: f64,
    /// New difficulty value [1, 10]
    pub difficulty: f64,
    /// Number of repetitions completed
    pub reps: u32,
    /// Interval until next review (days)
    pub interval_days: u32,
}

/// Compute next scheduling state after a review.
///
/// # Parameters
/// - `stability`: current stability (0.0 for new card)
/// - `difficulty`: current difficulty (0.0 for new card)
/// - `reps`: number of previous reviews
/// - `elapsed_days`: days since last review (0 for new card)
/// - `rating`: user's rating for this review
pub fn schedule(
    stability: f64,
    difficulty: f64,
    reps: u32,
    elapsed_days: f64,
    rating: Rating,
) -> ScheduleResult {
    if reps == 0 {
        // New card — use initial stability and difficulty
        let s = initial_stability(rating);
        let d = initial_difficulty(rating);
        let interval = next_interval(s);
        ScheduleResult {
            stability: s,
            difficulty: d,
            reps: 1,
            interval_days: interval,
        }
    } else {
        // Review of existing card
        let r = retrievability(elapsed_days, stability);
        let new_d = update_difficulty(difficulty, rating);

        let new_s = if rating == Rating::Again {
            stability_after_forgetting(new_d, stability, r)
        } else {
            stability_after_recall(new_d, stability, r, rating)
        };

        let interval = if rating == Rating::Again {
            1 // Always reschedule forgotten cards for tomorrow
        } else {
            next_interval(new_s)
        };

        ScheduleResult {
            stability: new_s.max(0.1),
            difficulty: new_d,
            reps: reps + 1,
            interval_days: interval,
        }
    }
}

/// Retrievability R(t, S) = (1 + FACTOR * t/S)^DECAY
fn retrievability(elapsed_days: f64, stability: f64) -> f64 {
    if stability <= 0.0 {
        return 0.0;
    }
    (1.0 + FACTOR * elapsed_days / stability).powf(DECAY)
}

/// Next review interval: solve for t where R(t, S) = DESIRED_RETENTION
/// → t = S * (R^(1/DECAY) - 1) / FACTOR
/// → With R=0.9, DECAY=-0.5: t = S (neat!)
fn next_interval(stability: f64) -> u32 {
    let interval = stability * (DESIRED_RETENTION.powf(1.0 / DECAY) - 1.0) / FACTOR;
    interval.round().max(1.0) as u32
}

/// Initial stability for a new card based on first rating: S_0(G) = W[G-1]
fn initial_stability(rating: Rating) -> f64 {
    W[rating as usize - 1]
}

/// Initial difficulty: D_0(G) = W[4] - exp(W[5] * (G-1)) + 1, clamped to [1, 10]
fn initial_difficulty(rating: Rating) -> f64 {
    let g = rating as usize as f64;
    let d = W[4] - (W[5] * (g - 1.0)).exp() + 1.0;
    d.clamp(1.0, 10.0)
}

/// Stability after successful recall.
///
/// S'_r(D, S, R, G) = S * e^W[8] * (11-D) * S^(-W[9]) * (e^(W[10]*(1-R)) - 1) + 1
/// multiplied by W[15] for Easy, or divided by W[16] for Hard.
fn stability_after_recall(
    difficulty: f64,
    stability: f64,
    retrievability: f64,
    rating: Rating,
) -> f64 {
    let base = stability
        * W[8].exp()
        * (11.0 - difficulty)
        * stability.powf(-W[9])
        * ((W[10] * (1.0 - retrievability)).exp() - 1.0)
        + 1.0;

    match rating {
        Rating::Easy => base * W[15],
        Rating::Hard => base / W[16],
        _ => base, // Good
    }
}

/// Stability after forgetting (Again rating).
///
/// S'_f(D, S, R) = W[11] * D^(-W[12]) * ((S+1)^W[13] - 1) * e^(W[14]*(1-R))
fn stability_after_forgetting(difficulty: f64, stability: f64, retrievability: f64) -> f64 {
    W[11]
        * difficulty.powf(-W[12])
        * ((stability + 1.0).powf(W[13]) - 1.0)
        * (W[14] * (1.0 - retrievability)).exp()
}

/// Update difficulty after review.
///
/// D'(D, G) = D - W[6] * (G - 3)
/// Then mean-reversion: D'' = W[7] * W[4] + (1 - W[7]) * D'
/// Clamped to [1, 10]
fn update_difficulty(difficulty: f64, rating: Rating) -> f64 {
    let g = rating as i32 as f64;
    let d_prime = difficulty - W[6] * (g - 3.0);
    let d_reverted = W[7] * W[4] + (1.0 - W[7]) * d_prime;
    d_reverted.clamp(1.0, 10.0)
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_card_good_rating() {
        let result = schedule(0.0, 0.0, 0, 0.0, Rating::Good);
        // Initial stability for Good = W[2] = 3.1262
        // With 90% retention, interval ≈ stability
        assert!((result.stability - 3.1262).abs() < 0.01);
        assert!(result.interval_days >= 3);
        assert_eq!(result.reps, 1);
        // Difficulty should be within [1, 10]
        assert!(result.difficulty >= 1.0);
        assert!(result.difficulty <= 10.0);
    }

    #[test]
    fn new_card_easy_rating() {
        let result = schedule(0.0, 0.0, 0, 0.0, Rating::Easy);
        // Initial stability for Easy = W[3] = 15.4722
        assert!((result.stability - 15.4722).abs() < 0.01);
        assert!(result.interval_days >= 15);
    }

    #[test]
    fn new_card_again_rating() {
        let result = schedule(0.0, 0.0, 0, 0.0, Rating::Again);
        // Initial stability for Again = W[0] = 0.4072 → interval = 1
        assert!(result.interval_days == 1);
        assert_eq!(result.reps, 1);
    }

    #[test]
    fn review_card_increases_stability() {
        // First review: Good
        let r1 = schedule(0.0, 0.0, 0, 0.0, Rating::Good);
        // Second review after interval days (at exact due date → R ≈ 0.9)
        let r2 = schedule(
            r1.stability,
            r1.difficulty,
            r1.reps,
            r1.interval_days as f64,
            Rating::Good,
        );
        // Stability should grow
        assert!(r2.stability > r1.stability);
        assert!(r2.interval_days > r1.interval_days);
    }

    #[test]
    fn again_rating_reschedules_tomorrow() {
        let r1 = schedule(0.0, 0.0, 0, 0.0, Rating::Good);
        // Forget the card
        let r2 = schedule(
            r1.stability,
            r1.difficulty,
            r1.reps,
            r1.interval_days as f64,
            Rating::Again,
        );
        assert_eq!(r2.interval_days, 1);
    }

    #[test]
    fn difficulty_decreases_for_easy() {
        let r1 = schedule(0.0, 0.0, 0, 0.0, Rating::Good);
        let base_difficulty = r1.difficulty;
        let r2 = schedule(
            r1.stability,
            r1.difficulty,
            r1.reps,
            r1.interval_days as f64,
            Rating::Easy,
        );
        // Easy should reduce difficulty
        assert!(r2.difficulty < base_difficulty || r2.difficulty == 1.0);
    }

    #[test]
    fn difficulty_clamped_to_range() {
        // Force a very high rating sequence to try to push difficulty out of range
        let mut stability = 0.0;
        let mut difficulty = 0.0;
        let mut reps = 0;
        for _ in 0..20 {
            let r = schedule(stability, difficulty, reps, 10.0, Rating::Again);
            assert!(r.difficulty >= 1.0, "difficulty {} < 1", r.difficulty);
            assert!(r.difficulty <= 10.0, "difficulty {} > 10", r.difficulty);
            stability = r.stability;
            difficulty = r.difficulty;
            reps = r.reps;
        }
    }

    #[test]
    fn interval_grows_over_multiple_good_reviews() {
        let mut stability = 0.0;
        let mut difficulty = 0.0;
        let mut reps = 0;
        let mut last_interval = 0;

        for _ in 0..5 {
            let elapsed = if reps == 0 { 0.0 } else { last_interval as f64 };
            let r = schedule(stability, difficulty, reps, elapsed, Rating::Good);
            if reps > 0 {
                assert!(
                    r.interval_days > last_interval,
                    "interval did not grow: {} → {}",
                    last_interval,
                    r.interval_days
                );
            }
            stability = r.stability;
            difficulty = r.difficulty;
            reps = r.reps;
            last_interval = r.interval_days;
        }
    }

    #[test]
    fn retrievability_at_due_date_is_90_percent() {
        // If elapsed_days == stability exactly, R should be ≈ 0.9
        let s = 10.0;
        let r = super::retrievability(s, s);
        assert!(
            (r - 0.9).abs() < 0.001,
            "retrievability = {}, expected ~0.9",
            r
        );
    }

    #[test]
    fn rating_from_u8() {
        assert_eq!(Rating::from_u8(1), Some(Rating::Again));
        assert_eq!(Rating::from_u8(2), Some(Rating::Hard));
        assert_eq!(Rating::from_u8(3), Some(Rating::Good));
        assert_eq!(Rating::from_u8(4), Some(Rating::Easy));
        assert_eq!(Rating::from_u8(0), None);
        assert_eq!(Rating::from_u8(5), None);
    }
}
