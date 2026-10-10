//! Statistics helpers (SPEC §5.10).
//!
//! All helpers ignore non-finite inputs (`NaN`, `±∞`), mirroring d3's habit of
//! skipping `NaN`, and return `None` when no usable value is left instead of
//! inventing a number.

use serde::{Deserialize, Serialize};

/// Copies the finite values out of `values` and sorts them ascending.
fn sorted_finite(values: &[f64]) -> Vec<f64> {
    let mut v: Vec<f64> = values.iter().copied().filter(|x| x.is_finite()).collect();
    v.sort_by(f64::total_cmp);
    v
}

/// Quantile of already sorted, finite, non-empty values (d3 `quantileSorted`).
fn quantile_sorted(sorted: &[f64], p: f64) -> f64 {
    let n = sorted.len();
    debug_assert!(n > 0);
    if p <= 0.0 || n < 2 {
        return sorted[0];
    }
    if p >= 1.0 {
        return sorted[n - 1];
    }
    let i = (n - 1) as f64 * p;
    let i0 = i.floor();
    let lo = sorted[i0 as usize];
    let hi = sorted[i0 as usize + 1];
    lo + (hi - lo) * (i - i0)
}

/// The `p`-quantile (`p` in `0.0..=1.0`) using linear interpolation between the
/// closest ranks, exactly like d3's `quantile` (R-7 / Excel `PERCENTILE.INC`).
///
/// `p` is clamped to `[0, 1]`. Returns `None` for an empty input (after
/// dropping non-finite values) or a `NaN` `p`.
pub fn percentile(values: &[f64], p: f64) -> Option<f64> {
    if p.is_nan() {
        return None;
    }
    let sorted = sorted_finite(values);
    if sorted.is_empty() {
        return None;
    }
    Some(quantile_sorted(&sorted, p))
}

/// The median (p50).
pub fn median(values: &[f64]) -> Option<f64> {
    percentile(values, 0.5)
}

/// Arithmetic mean of the finite values.
pub fn mean(values: &[f64]) -> Option<f64> {
    let finite: Vec<f64> = values.iter().copied().filter(|x| x.is_finite()).collect();
    if finite.is_empty() {
        None
    } else {
        Some(finite.iter().sum::<f64>() / finite.len() as f64)
    }
}

/// Jitter: the mean absolute difference between consecutive values, in input
/// order (Cloudflare's definition). Needs at least two finite values.
pub fn jitter(values: &[f64]) -> Option<f64> {
    let finite: Vec<f64> = values.iter().copied().filter(|x| x.is_finite()).collect();
    if finite.len() < 2 {
        return None;
    }
    let total: f64 = finite.windows(2).map(|w| (w[1] - w[0]).abs()).sum();
    Some(total / (finite.len() - 1) as f64)
}

/// Five-number summary used by the latency box plots.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BoxPlot {
    pub min: f64,
    pub q1: f64,
    pub median: f64,
    pub q3: f64,
    pub max: f64,
}

/// Computes min, first quartile, median, third quartile and max.
pub fn box_plot(values: &[f64]) -> Option<BoxPlot> {
    let sorted = sorted_finite(values);
    if sorted.is_empty() {
        return None;
    }
    Some(BoxPlot {
        min: sorted[0],
        q1: quantile_sorted(&sorted, 0.25),
        median: quantile_sorted(&sorted, 0.5),
        q3: quantile_sorted(&sorted, 0.75),
        max: sorted[sorted.len() - 1],
    })
}

/// Jain's fairness index `(Σx)² / (n·Σx²)` (SPEC §5.11).
///
/// 1.0 means a perfectly even split; `1/n` means one member got everything.
/// Returns `None` for no finite values, any negative value, or when every value
/// is zero (fairness is undefined when nobody received anything).
pub fn jain_fairness(values: &[f64]) -> Option<f64> {
    let finite: Vec<f64> = values.iter().copied().filter(|x| x.is_finite()).collect();
    if finite.is_empty() || finite.iter().any(|&x| x < 0.0) {
        return None;
    }
    let sum: f64 = finite.iter().sum();
    let sum_sq: f64 = finite.iter().map(|x| x * x).sum();
    if sum_sq == 0.0 {
        return None;
    }
    Some((sum * sum) / (finite.len() as f64 * sum_sq))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn percentile_matches_d3_quantile() {
        // d3.quantile([0, 10, 30], p)
        let v = [0.0, 10.0, 30.0];
        assert_eq!(percentile(&v, 0.0), Some(0.0));
        assert_eq!(percentile(&v, 0.25), Some(5.0));
        assert_eq!(percentile(&v, 0.5), Some(10.0));
        assert_eq!(percentile(&v, 0.75), Some(20.0));
        assert_eq!(percentile(&v, 1.0), Some(30.0));
        // d3 docs: quantile([3, 1, 2, 4, 5], 0.9) sorts first -> 4.6
        assert!(close(
            percentile(&[3.0, 1.0, 2.0, 4.0, 5.0], 0.9).unwrap(),
            4.6
        ));
        // Ten values 1..=10: p90 = 9.1, p50 = 5.5
        let ten: Vec<f64> = (1..=10).map(f64::from).collect();
        assert!(close(percentile(&ten, 0.9).unwrap(), 9.1));
        assert!(close(percentile(&ten, 0.5).unwrap(), 5.5));
    }

    #[test]
    fn percentile_clamps_p_and_handles_edge_cases() {
        let v = [1.0, 2.0, 3.0];
        assert_eq!(percentile(&v, -1.0), Some(1.0));
        assert_eq!(percentile(&v, 2.0), Some(3.0));
        assert_eq!(percentile(&v, f64::NAN), None);
        assert_eq!(percentile(&[], 0.5), None);
        assert_eq!(percentile(&[7.0], 0.9), Some(7.0));
        assert_eq!(percentile(&[f64::NAN, f64::INFINITY], 0.5), None);
        assert_eq!(percentile(&[f64::NAN, 4.0, 2.0], 0.5), Some(3.0));
    }

    #[test]
    fn median_even_and_odd() {
        assert_eq!(median(&[5.0, 1.0, 3.0]), Some(3.0));
        assert_eq!(median(&[4.0, 1.0, 3.0, 2.0]), Some(2.5));
        assert_eq!(median(&[]), None);
    }

    #[test]
    fn mean_works() {
        assert_eq!(mean(&[1.0, 2.0, 6.0]), Some(3.0));
        assert_eq!(mean(&[f64::NAN]), None);
    }

    #[test]
    fn jitter_is_mean_abs_consecutive_diff() {
        // |12-10| + |11-12| + |15-11| = 2 + 1 + 4 = 7 over 3 diffs
        assert!(close(jitter(&[10.0, 12.0, 11.0, 15.0]).unwrap(), 7.0 / 3.0));
        assert_eq!(jitter(&[5.0, 5.0, 5.0]), Some(0.0));
        assert_eq!(jitter(&[1.0]), None);
        assert_eq!(jitter(&[]), None);
        assert_eq!(jitter(&[1.0, f64::NAN]), None);
        assert_eq!(jitter(&[1.0, f64::NAN, 3.0]), Some(2.0));
    }

    #[test]
    fn box_plot_five_numbers() {
        let b = box_plot(&[9.0, 1.0, 5.0, 3.0, 7.0]).unwrap();
        assert_eq!(
            b,
            BoxPlot {
                min: 1.0,
                q1: 3.0,
                median: 5.0,
                q3: 7.0,
                max: 9.0
            }
        );
        let one = box_plot(&[2.0]).unwrap();
        assert_eq!(
            (one.min, one.q1, one.median, one.q3, one.max),
            (2.0, 2.0, 2.0, 2.0, 2.0)
        );
        assert_eq!(box_plot(&[]), None);
    }

    #[test]
    fn jain_fairness_known_values() {
        assert!(close(jain_fairness(&[100.0, 100.0, 100.0]).unwrap(), 1.0));
        // One of four gets everything: 1/n
        assert!(close(jain_fairness(&[0.0, 0.0, 0.0, 40.0]).unwrap(), 0.25));
        // (1+2+3)^2 / (3 * 14) = 36 / 42
        assert!(close(jain_fairness(&[1.0, 2.0, 3.0]).unwrap(), 36.0 / 42.0));
        assert_eq!(jain_fairness(&[5.0]), Some(1.0));
        assert_eq!(jain_fairness(&[]), None);
        assert_eq!(jain_fairness(&[0.0, 0.0]), None);
        assert_eq!(jain_fairness(&[-1.0, 2.0]), None);
    }
}
