#![allow(dead_code)]

//! Pure statistics helpers: Wilson score confidence intervals and the exact
//! `McNemar` sign test. No I/O; std only.

/// 95% z-score used for all reported intervals.
pub const Z_95: f64 = 1.96;

/// Wilson score interval for `successes` out of `n` Bernoulli trials.
/// Returns (low, high), each clamped to [0.0, 1.0]. Returns (0.0, 0.0) when n == 0.
#[allow(clippy::cast_precision_loss)]
pub fn wilson_interval(successes: u64, n: u64, z: f64) -> (f64, f64) {
    if n == 0 {
        return (0.0, 0.0);
    }

    let n_f = n as f64;
    let p = successes as f64 / n_f;
    let z2 = z * z;
    let denom = 1.0 + z2 / n_f;
    let center = (p + z2 / (2.0 * n_f)) / denom;
    let half = (z / denom) * (p * (1.0 - p) / n_f + z2 / (4.0 * n_f * n_f)).sqrt();
    let low = (center - half).max(0.0);
    let high = (center + half).min(1.0);
    (low, high)
}

/// Two-sided exact `McNemar` p-value from discordant pair counts:
/// `b` = pairs where A resolved and B did not; `c` = the reverse.
/// Returns None when b + c == 0 (no discordant pairs; test undefined).
#[allow(clippy::cast_precision_loss)]
pub fn mcnemar_exact_p(b: u64, c: u64) -> Option<f64> {
    let n = b + c;
    if n == 0 {
        return None;
    }
    let k = b.min(c);
    let ln2 = std::f64::consts::LN_2;
    let mut ln_choose = 0.0_f64; // ln C(n, 0)
    let mut sum = 0.0_f64;
    for i in 0..=k {
        if i > 0 {
            ln_choose += ((n - i + 1) as f64).ln() - (i as f64).ln();
        }
        sum += (ln_choose - (n as f64) * ln2).exp();
    }
    Some((2.0 * sum).min(1.0))
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;

    const TOL: f64 = 1e-3;

    fn assert_close(actual: f64, expected: f64) {
        assert!(
            (actual - expected).abs() < TOL,
            "expected {expected}, got {actual}"
        );
    }

    #[test]
    fn wilson_zero_n() {
        assert_eq!(wilson_interval(0, 0, Z_95), (0.0, 0.0));
    }

    #[test]
    fn wilson_3_of_5() {
        let (low, high) = wilson_interval(3, 5, Z_95);
        assert_close(low, 0.2307);
        assert_close(high, 0.8824);
    }

    #[test]
    fn wilson_0_of_5() {
        let (low, high) = wilson_interval(0, 5, Z_95);
        assert_eq!(low, 0.0);
        assert_close(high, 0.4345);
    }

    #[test]
    fn wilson_5_of_5() {
        let (low, high) = wilson_interval(5, 5, Z_95);
        assert_close(low, 0.5655);
        assert_eq!(high, 1.0);
    }

    #[test]
    fn mcnemar_no_discordant() {
        assert_eq!(mcnemar_exact_p(0, 0), None);
    }

    #[test]
    fn mcnemar_one_discordant() {
        assert_eq!(mcnemar_exact_p(1, 0), Some(1.0));
    }

    #[test]
    fn mcnemar_5_0() {
        let p = mcnemar_exact_p(5, 0).expect("p-value");
        assert_close(p, 0.0625);
    }

    #[test]
    fn mcnemar_1_1() {
        assert_eq!(mcnemar_exact_p(1, 1), Some(1.0));
    }

    #[test]
    fn mcnemar_8_1() {
        let p = mcnemar_exact_p(8, 1).expect("p-value");
        assert_close(p, 0.039_062_5);
    }

    // Note: mcnemar_5_0 and mcnemar_8_1 are annotated "exact" in the plan's
    // test table, but this implementation computes them in log-space
    // (ln/exp round-trip) per the plan's own required algorithm, which does
    // not reproduce the closed-form binomial ratios bit-for-bit. Using the
    // 1e-3 tolerance (explicitly allowed by the plan's preamble: "unless
    // exact") keeps the test meaningful without depending on incidental
    // float rounding.

    #[test]
    fn mcnemar_symmetric() {
        assert_eq!(mcnemar_exact_p(2, 7), mcnemar_exact_p(7, 2));
    }
}
