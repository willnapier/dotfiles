use anyhow::{ensure, Result};
use rand::rngs::OsRng;
use rand::RngCore;

use crate::gf256;

/// Split a secret into `n` shares requiring `k` to reconstruct.
///
/// Returns `n` shares, each formatted as:
///   [x_coordinate, y_1, y_2, ..., y_secret_len]
///
/// x coordinates are 1..=n (never 0, since f(0) = secret).
pub fn split(secret: &[u8], k: u8, n: u8) -> Result<Vec<Vec<u8>>> {
    ensure!(!secret.is_empty(), "secret must not be empty");
    ensure!(k >= 1, "threshold must be at least 1");
    ensure!(n >= k, "shares must be >= threshold");
    ensure!(n > 0, "shares must be at least 1");

    let mut shares: Vec<Vec<u8>> = (1..=n)
        .map(|x| {
            let mut s = Vec::with_capacity(1 + secret.len());
            s.push(x);
            s
        })
        .collect();

    let mut coeffs = vec![0u8; k as usize];

    for &secret_byte in secret {
        coeffs[0] = secret_byte;
        if k > 1 {
            OsRng.fill_bytes(&mut coeffs[1..]);
        }

        for share in &mut shares {
            let x = share[0];
            let y = eval_polynomial(&coeffs, x);
            share.push(y);
        }
    }

    Ok(shares)
}

/// Reconstruct a secret from `k` or more shares.
///
/// Each share must be [x_coordinate, y_1, y_2, ..., y_n].
/// All shares must have the same length.
pub fn reconstruct(shares: &[Vec<u8>]) -> Result<Vec<u8>> {
    ensure!(!shares.is_empty(), "need at least one share");
    let share_len = shares[0].len();
    ensure!(share_len > 1, "shares contain no secret data");

    for (i, share) in shares.iter().enumerate() {
        ensure!(
            share.len() == share_len,
            "share {} has length {}, expected {}",
            i,
            share.len(),
            share_len
        );
    }

    let xs: Vec<u8> = shares.iter().map(|s| s[0]).collect();

    // Check for duplicate x coordinates
    let mut seen = [false; 256];
    for &x in &xs {
        ensure!(!seen[x as usize], "duplicate x coordinate: {}", x);
        seen[x as usize] = true;
    }

    let secret_len = share_len - 1;
    let mut secret = Vec::with_capacity(secret_len);

    for byte_idx in 0..secret_len {
        let ys: Vec<u8> = shares.iter().map(|s| s[1 + byte_idx]).collect();
        secret.push(lagrange_at_zero(&xs, &ys));
    }

    Ok(secret)
}

/// Evaluate polynomial at x using Horner's method in GF(256).
/// coeffs = [a_0, a_1, ..., a_{k-1}]
/// f(x) = a_0 + a_1*x + a_2*x^2 + ...
fn eval_polynomial(coeffs: &[u8], x: u8) -> u8 {
    let mut result = 0u8;
    for &coeff in coeffs.iter().rev() {
        result = gf256::add(gf256::mul(result, x), coeff);
    }
    result
}

/// Lagrange interpolation at x=0 in GF(256).
fn lagrange_at_zero(xs: &[u8], ys: &[u8]) -> u8 {
    let k = xs.len();
    let mut result = 0u8;

    for i in 0..k {
        let mut numerator = 1u8;
        let mut denominator = 1u8;
        for j in 0..k {
            if i == j {
                continue;
            }
            numerator = gf256::mul(numerator, xs[j]);
            denominator = gf256::mul(denominator, gf256::add(xs[i], xs[j]));
        }
        let basis = gf256::div(numerator, denominator);
        result = gf256::add(result, gf256::mul(ys[i], basis));
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_2_of_3() {
        let secret = b"hello world";
        let shares = split(secret, 2, 3).unwrap();
        assert_eq!(shares.len(), 3);

        // Any 2 shares should reconstruct
        for i in 0..3 {
            for j in (i + 1)..3 {
                let subset = vec![shares[i].clone(), shares[j].clone()];
                let recovered = reconstruct(&subset).unwrap();
                assert_eq!(recovered, secret);
            }
        }
    }

    #[test]
    fn roundtrip_3_of_5() {
        let secret = b"a longer secret with more bytes for good measure!";
        let shares = split(secret, 3, 5).unwrap();
        assert_eq!(shares.len(), 5);

        // Any 3 shares should work
        let subset = vec![shares[0].clone(), shares[2].clone(), shares[4].clone()];
        let recovered = reconstruct(&subset).unwrap();
        assert_eq!(recovered, secret);
    }

    #[test]
    fn roundtrip_all_shares() {
        let secret = b"using all shares";
        let shares = split(secret, 2, 3).unwrap();
        let recovered = reconstruct(&shares).unwrap();
        assert_eq!(recovered, secret);
    }

    #[test]
    fn roundtrip_threshold_equals_shares() {
        let secret = b"tight";
        let shares = split(secret, 3, 3).unwrap();
        let recovered = reconstruct(&shares).unwrap();
        assert_eq!(recovered, secret);
    }

    #[test]
    fn roundtrip_threshold_1() {
        let secret = b"no splitting needed";
        let shares = split(secret, 1, 3).unwrap();
        // Each single share should reconstruct
        for share in &shares {
            let recovered = reconstruct(&[share.clone()]).unwrap();
            assert_eq!(recovered, secret);
        }
    }

    #[test]
    fn single_byte_secret() {
        let secret = &[42u8];
        let shares = split(secret, 2, 3).unwrap();
        let subset = vec![shares[0].clone(), shares[2].clone()];
        let recovered = reconstruct(&subset).unwrap();
        assert_eq!(recovered, secret);
    }

    #[test]
    fn insufficient_shares_gives_wrong_result() {
        let secret = b"secret";
        let shares = split(secret, 3, 5).unwrap();
        // Only 2 shares with threshold 3 — should NOT recover correctly
        // (information-theoretically secure, so result is random)
        let subset = vec![shares[0].clone(), shares[1].clone()];
        let recovered = reconstruct(&subset).unwrap();
        // With overwhelming probability, this won't match
        assert_ne!(recovered, secret);
    }

    #[test]
    fn large_secret() {
        let secret: Vec<u8> = (0..1024).map(|i| (i % 256) as u8).collect();
        let shares = split(&secret, 2, 3).unwrap();
        let subset = vec![shares[0].clone(), shares[1].clone()];
        let recovered = reconstruct(&subset).unwrap();
        assert_eq!(recovered, secret);
    }

    #[test]
    fn error_empty_secret() {
        assert!(split(b"", 2, 3).is_err());
    }

    #[test]
    fn error_threshold_exceeds_shares() {
        assert!(split(b"x", 3, 2).is_err());
    }

    #[test]
    fn error_no_shares_to_reconstruct() {
        let empty: Vec<Vec<u8>> = vec![];
        assert!(reconstruct(&empty).is_err());
    }

    #[test]
    fn error_duplicate_x_coordinates() {
        let share = vec![1u8, 42, 43];
        assert!(reconstruct(&[share.clone(), share]).is_err());
    }

    #[test]
    fn share_format() {
        let secret = b"AB";
        let shares = split(secret, 2, 3).unwrap();
        for (i, share) in shares.iter().enumerate() {
            assert_eq!(share[0], (i + 1) as u8); // x coordinate is 1-indexed
            assert_eq!(share.len(), 3); // x + 2 secret bytes
        }
    }
}
