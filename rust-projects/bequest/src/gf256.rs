/// GF(256) finite field arithmetic using the AES irreducible polynomial.
///
/// Polynomial: x^8 + x^4 + x^3 + x + 1 (0x11b)
/// Generator: 3 (primitive element, as used in AES)
///
/// These exact tables are embedded in the reconstruction HTML page's
/// JavaScript. Any change here must be mirrored there — the page generator
/// in page.rs reads these tables via exp_table_json() / log_table_json().
const fn generate_tables() -> ([u8; 256], [u8; 256]) {
    let mut exp = [0u8; 256];
    let mut log = [0u8; 256];
    let mut val: u16 = 1;
    let mut i = 0;
    while i < 255 {
        exp[i] = val as u8;
        log[val as usize] = i as u8;
        // Multiply by generator 3 in GF(256):
        // val * 3 = val * (2 + 1) = (val * 2) XOR val
        let mut doubled = val << 1;
        if doubled & 0x100 != 0 {
            doubled ^= 0x11b;
        }
        val = doubled ^ val;
        i += 1;
    }
    exp[255] = exp[0]; // g^255 = g^0 = 1, convenience wrap
    (exp, log)
}

static TABLES: ([u8; 256], [u8; 256]) = generate_tables();

pub static EXP: &[u8; 256] = &TABLES.0;
pub static LOG: &[u8; 256] = &TABLES.1;

#[inline]
pub fn add(a: u8, b: u8) -> u8 {
    a ^ b
}

#[inline]
pub fn mul(a: u8, b: u8) -> u8 {
    if a == 0 || b == 0 {
        return 0;
    }
    EXP[((LOG[a as usize] as u16 + LOG[b as usize] as u16) % 255) as usize]
}

#[inline]
pub fn div(a: u8, b: u8) -> u8 {
    assert!(b != 0, "division by zero in GF(256)");
    if a == 0 {
        return 0;
    }
    let diff = (LOG[a as usize] as i16 - LOG[b as usize] as i16 + 255) % 255;
    EXP[diff as usize]
}

/// EXP table as a JS array literal for embedding in the reconstruction page.
pub fn exp_table_json() -> String {
    format!(
        "[{}]",
        EXP.iter()
            .map(|b| b.to_string())
            .collect::<Vec<_>>()
            .join(",")
    )
}

/// LOG table as a JS array literal for embedding in the reconstruction page.
pub fn log_table_json() -> String {
    format!(
        "[{}]",
        LOG.iter()
            .map(|b| b.to_string())
            .collect::<Vec<_>>()
            .join(",")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exp_log_roundtrip() {
        for i in 1u16..=255 {
            assert_eq!(EXP[LOG[i as usize] as usize], i as u8);
        }
    }

    #[test]
    fn mul_identity() {
        for a in 0..=255u8 {
            assert_eq!(mul(a, 1), a);
        }
    }

    #[test]
    fn mul_zero() {
        for a in 0..=255u8 {
            assert_eq!(mul(a, 0), 0);
        }
    }

    #[test]
    fn add_self_is_zero() {
        for a in 0..=255u8 {
            assert_eq!(add(a, a), 0);
        }
    }

    #[test]
    fn mul_div_roundtrip() {
        for a in 0..=255u8 {
            for b in 1..=255u8 {
                assert_eq!(div(mul(a, b), b), a);
            }
        }
    }

    #[test]
    fn known_values() {
        // g^0 = 1
        assert_eq!(EXP[0], 1);
        // g^1 = 3 (generator)
        assert_eq!(EXP[1], 3);
        // g^2 = 3*3 = 5 in GF(256)
        assert_eq!(EXP[2], 5);
        // g^255 wraps to g^0 = 1
        assert_eq!(EXP[255], 1);
    }

    #[test]
    fn mul_commutativity() {
        for a in 0..=255u8 {
            for b in 0..=255u8 {
                assert_eq!(mul(a, b), mul(b, a));
            }
        }
    }
}
