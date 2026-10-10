//! Platform-free chart geometry: linear scales, "nice" ticks, SVG paths and
//! deterministic dot spreading. Unit-tested natively.

use crate::model::units::Unit;

/// Maps a domain linearly onto a range (either may be reversed).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Linear {
    pub d0: f64,
    pub d1: f64,
    pub r0: f64,
    pub r1: f64,
}

impl Linear {
    pub fn new(domain: (f64, f64), range: (f64, f64)) -> Self {
        Self {
            d0: domain.0,
            d1: domain.1,
            r0: range.0,
            r1: range.1,
        }
    }

    pub fn map(&self, v: f64) -> f64 {
        let span = self.d1 - self.d0;
        if span == 0.0 || !span.is_finite() {
            return self.r0;
        }
        self.r0 + (v - self.d0) / span * (self.r1 - self.r0)
    }

    /// Inverse of [`map`](Self::map).
    pub fn invert(&self, r: f64) -> f64 {
        let span = self.r1 - self.r0;
        if span == 0.0 {
            return self.d0;
        }
        self.d0 + (r - self.r0) / span * (self.d1 - self.d0)
    }
}

/// A 1/2/5 × 10ⁿ step giving about `target` intervals over `span`.
pub fn nice_step(span: f64, target: usize) -> f64 {
    if !(span.is_finite() && span > 0.0) {
        return 1.0;
    }
    let raw = span / target.max(1) as f64;
    let mag = 10f64.powf(raw.log10().floor());
    let norm = raw / mag;
    // d3's thresholds: pick the step whose error ratio is smallest.
    let nice = if norm >= 7.07 {
        10.0
    } else if norm >= 3.16 {
        5.0
    } else if norm >= 1.41 {
        2.0
    } else {
        1.0
    };
    nice * mag
}

/// Ticks from 0 to a nice maximum ≥ `max` (about `target` intervals).
/// The last tick is the axis maximum.
pub fn ticks_from_zero(max: f64, target: usize) -> Vec<f64> {
    let max = if max.is_finite() && max > 0.0 {
        max
    } else {
        1.0
    };
    let step = nice_step(max, target);
    let n = (max / step - 1e-9).ceil().max(1.0) as usize;
    (0..=n).map(|i| round_tick(i as f64 * step)).collect()
}

/// Ticks between `min` and `max` (both included when they fall on a step),
/// for a domain that doesn't start at 0.
pub fn ticks_between(min: f64, max: f64, target: usize) -> Vec<f64> {
    if !(min.is_finite() && max.is_finite()) || max <= min {
        return vec![min];
    }
    let step = nice_step(max - min, target);
    let first = (min / step - 1e-9).ceil() as i64;
    let last = (max / step + 1e-9).floor() as i64;
    (first..=last)
        .map(|i| round_tick(i as f64 * step))
        .collect()
}

/// Removes float noise such as 0.30000000000000004.
fn round_tick(v: f64) -> f64 {
    (v * 1e9).round() / 1e9
}

/// Short tick label: `0`, `250`, `2.5`, `1.25`, `1.5K`.
pub fn tick_label(v: f64) -> String {
    if v.abs() >= 10_000.0 {
        let k = v / 1000.0;
        return if (k - k.round()).abs() < 1e-9 {
            format!("{}K", k.round() as i64)
        } else {
            format!("{k:.1}K")
        };
    }
    if (v - v.round()).abs() < 1e-9 {
        format!("{}", v.round() as i64)
    } else {
        let s = format!("{v:.2}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

/// The display scale for a throughput axis whose largest value is `max_bps`:
/// bits per second per display unit, and the unit label. Switches to G at
/// ≥ 1000 of the base unit, like [`crate::model::units::format_rate`].
pub fn rate_axis(max_bps: f64, unit: Unit) -> (f64, &'static str) {
    let per = match unit {
        Unit::Mbps => 1e6,
        Unit::MBps => 8e6,
    };
    if max_bps.is_finite() && max_bps / per >= 1000.0 {
        (per * 1000.0, unit.large_label())
    } else {
        (per, unit.label())
    }
}

/// `M x y L x y …` through `points` (empty for no points).
pub fn line_path(points: &[(f64, f64)]) -> String {
    let mut s = String::new();
    for (i, (x, y)) in points.iter().enumerate() {
        s.push(if i == 0 { 'M' } else { 'L' });
        s.push_str(&format!("{:.1} {:.1}", x, y));
    }
    s
}

/// Closed area under `points` down to `baseline`.
pub fn area_path(points: &[(f64, f64)], baseline: f64) -> String {
    let (Some(first), Some(last)) = (points.first(), points.last()) else {
        return String::new();
    };
    format!(
        "{}L{:.1} {:.1}L{:.1} {:.1}Z",
        line_path(points),
        last.0,
        baseline,
        first.0,
        baseline
    )
}

/// Points of a sparkline of `values` in a `w` × `h` box (y grows down), with
/// `pad` px kept free at top and bottom. The y domain starts at 0 when
/// `from_zero`, else at the minimum value.
pub fn sparkline_points(
    values: &[f64],
    w: f64,
    h: f64,
    pad: f64,
    from_zero: bool,
) -> Vec<(f64, f64)> {
    let vals: Vec<f64> = values.iter().copied().filter(|v| v.is_finite()).collect();
    if vals.is_empty() {
        return Vec::new();
    }
    let max = vals.iter().copied().fold(f64::MIN, f64::max);
    let min = if from_zero {
        0.0
    } else {
        vals.iter().copied().fold(f64::MAX, f64::min)
    };
    let y = Linear::new(
        (min, if max > min { max } else { min + 1.0 }),
        (h - pad, pad),
    );
    let n = vals.len();
    let x = |i: usize| {
        if n == 1 {
            w / 2.0
        } else {
            i as f64 / (n - 1) as f64 * w
        }
    };
    vals.iter()
        .enumerate()
        .map(|(i, &v)| (x(i), y.map(v)))
        .collect()
}

/// A deterministic offset in `-1.0..=1.0` for the `i`-th dot of a band, so
/// dots in a dot plot spread out without randomness (golden-ratio sequence).
pub fn spread(i: usize) -> f64 {
    if i == 0 {
        return 0.0;
    }
    let f = (i as f64 * 0.618_033_988_75).fract();
    f * 2.0 - 1.0
}

/// Size label for payloads: `100 kB`, `1 MB`, `1.5 MB`, `1 GB`.
pub fn format_bytes(bytes: u64) -> String {
    let b = bytes as f64;
    let (v, unit) = if b >= 1e9 {
        (b / 1e9, "GB")
    } else if b >= 1e6 {
        (b / 1e6, "MB")
    } else if b >= 1e3 {
        (b / 1e3, "kB")
    } else {
        (b, "B")
    };
    if (v - v.round()).abs() < 1e-9 {
        format!("{} {unit}", v.round() as u64)
    } else {
        format!("{v:.1} {unit}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linear_maps_and_inverts() {
        let s = Linear::new((0.0, 100.0), (200.0, 0.0));
        assert_eq!(s.map(0.0), 200.0);
        assert_eq!(s.map(50.0), 100.0);
        assert_eq!(s.invert(100.0), 50.0);
        let flat = Linear::new((5.0, 5.0), (0.0, 10.0));
        assert_eq!(flat.map(5.0), 0.0);
    }

    #[test]
    fn nice_ticks() {
        assert_eq!(nice_step(100.0, 4), 20.0);
        assert_eq!(nice_step(940.0, 4), 200.0);
        assert_eq!(nice_step(0.0, 4), 1.0);
        assert_eq!(
            ticks_from_zero(940.0, 4),
            vec![0.0, 200.0, 400.0, 600.0, 800.0, 1000.0]
        );
        assert_eq!(
            ticks_from_zero(87.0, 4),
            vec![0.0, 20.0, 40.0, 60.0, 80.0, 100.0]
        );
        assert_eq!(ticks_from_zero(0.3, 3), vec![0.0, 0.1, 0.2, 0.3]);
        assert_eq!(
            ticks_from_zero(f64::NAN, 4),
            vec![0.0, 0.2, 0.4, 0.6, 0.8, 1.0]
        );
        assert_eq!(ticks_between(2.3, 9.1, 3), vec![4.0, 6.0, 8.0]);
        assert_eq!(ticks_between(5.0, 5.0, 3), vec![5.0]);
    }

    #[test]
    fn tick_labels() {
        assert_eq!(tick_label(0.0), "0");
        assert_eq!(tick_label(250.0), "250");
        assert_eq!(tick_label(2.5), "2.5");
        assert_eq!(tick_label(1.25), "1.25");
        assert_eq!(tick_label(0.1 + 0.2), "0.3");
        assert_eq!(tick_label(15_000.0), "15K");
        assert_eq!(tick_label(12_500.0), "12.5K");
    }

    #[test]
    fn rate_axes() {
        assert_eq!(rate_axis(940e6, Unit::Mbps), (1e6, "Mbps"));
        assert_eq!(rate_axis(2.4e9, Unit::Mbps), (1e9, "Gbps"));
        assert_eq!(rate_axis(940e6, Unit::MBps), (8e6, "MB/s"));
        assert_eq!(rate_axis(9e9, Unit::MBps), (8e9, "GB/s"));
        assert_eq!(rate_axis(f64::NAN, Unit::Mbps), (1e6, "Mbps"));
    }

    #[test]
    fn paths() {
        assert_eq!(line_path(&[]), "");
        assert_eq!(line_path(&[(0.0, 1.0), (2.0, 3.5)]), "M0.0 1.0L2.0 3.5");
        assert_eq!(
            area_path(&[(0.0, 1.0), (2.0, 3.0)], 10.0),
            "M0.0 1.0L2.0 3.0L2.0 10.0L0.0 10.0Z"
        );
        assert_eq!(area_path(&[], 10.0), "");
    }

    #[test]
    fn sparklines() {
        let p = sparkline_points(&[0.0, 5.0, 10.0], 100.0, 20.0, 2.0, true);
        assert_eq!(p, vec![(0.0, 18.0), (50.0, 10.0), (100.0, 2.0)]);
        let p = sparkline_points(&[3.0], 100.0, 20.0, 2.0, false);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].0, 50.0);
        assert!(sparkline_points(&[f64::NAN], 10.0, 10.0, 0.0, true).is_empty());
        // Flat series stay inside the box.
        let p = sparkline_points(&[4.0, 4.0], 10.0, 10.0, 1.0, false);
        assert!(p.iter().all(|&(_, y)| (1.0..=9.0).contains(&y)));
    }

    #[test]
    fn spread_is_bounded_and_deterministic() {
        assert_eq!(spread(0), 0.0);
        for i in 0..200 {
            let s = spread(i);
            assert!((-1.0..=1.0).contains(&s));
            assert_eq!(s, spread(i));
        }
        assert_ne!(spread(1), spread(2));
    }

    #[test]
    fn bytes_labels() {
        assert_eq!(format_bytes(100_000), "100 kB");
        assert_eq!(format_bytes(250_000_000), "250 MB");
        assert_eq!(format_bytes(1_500_000), "1.5 MB");
        assert_eq!(format_bytes(1_000_000_000), "1 GB");
        assert_eq!(format_bytes(12), "12 B");
    }
}
