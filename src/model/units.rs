//! Display units and number formatting (SPEC §7.6).
//!
//! Results are always stored in bits per second; these helpers only change
//! how values are shown. Formatters return the value and its unit separately
//! so tiles can style them differently.

use serde::{Deserialize, Serialize};

/// Placeholder shown for missing or invalid values.
pub const PLACEHOLDER: &str = "—";

/// Rate display unit chosen with the units toggle.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Unit {
    /// Megabits per second (auto-scales to Gbps).
    #[default]
    #[serde(rename = "Mbps")]
    Mbps,
    /// Megabytes per second (auto-scales to GB/s).
    #[serde(rename = "MB/s")]
    MBps,
}

impl Unit {
    /// Base unit label (`"Mbps"` or `"MB/s"`).
    pub fn label(self) -> &'static str {
        match self {
            Unit::Mbps => "Mbps",
            Unit::MBps => "MB/s",
        }
    }

    /// Scaled unit label (`"Gbps"` or `"GB/s"`).
    pub fn large_label(self) -> &'static str {
        match self {
            Unit::Mbps => "Gbps",
            Unit::MBps => "GB/s",
        }
    }

    /// Converts bits per second to this (unscaled) unit.
    pub fn from_bps(self, bps: f64) -> f64 {
        match self {
            Unit::Mbps => bps / 1e6,
            Unit::MBps => bps / 8e6,
        }
    }

    /// The other unit (for the toggle).
    pub fn toggled(self) -> Self {
        match self {
            Unit::Mbps => Unit::MBps,
            Unit::MBps => Unit::Mbps,
        }
    }
}

/// Formats `v` with a number of decimals chosen by `decimals(v)`, re-checking
/// after rounding so that e.g. 9.996 becomes "10.0" rather than "10.00".
fn fmt_adaptive(v: f64, decimals: impl Fn(f64) -> usize) -> String {
    let d = decimals(v);
    let s = format!("{v:.d$}");
    let rounded: f64 = s.parse().unwrap_or(v);
    let d2 = decimals(rounded.abs());
    if d2 == d {
        s
    } else {
        format!("{rounded:.d2$}")
    }
}

/// About three significant digits: 0.98, 9.84, 98.4, 984.
fn sig3(v: f64) -> String {
    fmt_adaptive(v, |x| {
        if x >= 100.0 {
            0
        } else if x >= 10.0 {
            1
        } else {
            2
        }
    })
}

/// Formats a rate given in bps, auto-scaling to Gbps / GB/s at ≥ 1000.
///
/// `format_rate(984e6, Unit::Mbps) == ("984".into(), "Mbps")`,
/// `format_rate(1.234e9, Unit::Mbps) == ("1.23".into(), "Gbps")`.
/// Negative or non-finite input gives [`PLACEHOLDER`].
pub fn format_rate(bps: f64, unit: Unit) -> (String, &'static str) {
    if !bps.is_finite() || bps < 0.0 {
        return (PLACEHOLDER.to_string(), unit.label());
    }
    let v = unit.from_bps(bps);
    let s = sig3(v);
    if v >= 1000.0 || s.parse::<f64>().is_ok_and(|r| r >= 1000.0) {
        (sig3(v / 1000.0), unit.large_label())
    } else {
        (s, unit.label())
    }
}

/// Formats milliseconds with about three significant digits: "0.42", "2.51",
/// "12.3", "123".
pub fn format_ms(ms: f64) -> (String, &'static str) {
    if !ms.is_finite() || ms < 0.0 {
        return (PLACEHOLDER.to_string(), "ms");
    }
    (sig3(ms), "ms")
}

/// Formats a ratio (`0.0..=1.0`) as a percentage: "0", "0.25", "1.5", "25".
pub fn format_percent(ratio: f64) -> (String, &'static str) {
    if !ratio.is_finite() || ratio < 0.0 {
        return (PLACEHOLDER.to_string(), "%");
    }
    let pct = ratio * 100.0;
    if pct == 0.0 {
        return ("0".to_string(), "%");
    }
    let s = fmt_adaptive(pct, |x| {
        if x >= 10.0 {
            0
        } else if x >= 1.0 {
            1
        } else {
            2
        }
    });
    (s, "%")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rate(bps: f64, unit: Unit) -> (String, &'static str) {
        format_rate(bps, unit)
    }

    #[test]
    fn rate_mbps_three_significant_digits() {
        assert_eq!(rate(9.84e6, Unit::Mbps), ("9.84".into(), "Mbps"));
        assert_eq!(rate(98.4e6, Unit::Mbps), ("98.4".into(), "Mbps"));
        assert_eq!(rate(984e6, Unit::Mbps), ("984".into(), "Mbps"));
        assert_eq!(rate(1.234e9, Unit::Mbps), ("1.23".into(), "Gbps"));
        assert_eq!(rate(12.3e9, Unit::Mbps), ("12.3".into(), "Gbps"));
        assert_eq!(rate(0.5e6, Unit::Mbps), ("0.50".into(), "Mbps"));
        assert_eq!(rate(0.0, Unit::Mbps), ("0.00".into(), "Mbps"));
    }

    #[test]
    fn rate_rounding_crosses_bands() {
        assert_eq!(rate(9.996e6, Unit::Mbps), ("10.0".into(), "Mbps"));
        assert_eq!(rate(99.96e6, Unit::Mbps), ("100".into(), "Mbps"));
        assert_eq!(rate(999.6e6, Unit::Mbps), ("1.00".into(), "Gbps"));
        assert_eq!(rate(1e9, Unit::Mbps), ("1.00".into(), "Gbps"));
        assert_eq!(rate(999.4e6, Unit::Mbps), ("999".into(), "Mbps"));
    }

    #[test]
    fn rate_megabytes() {
        assert_eq!(rate(800e6, Unit::MBps), ("100".into(), "MB/s"));
        assert_eq!(rate(80e6, Unit::MBps), ("10.0".into(), "MB/s"));
        assert_eq!(rate(9.6e9, Unit::MBps), ("1.20".into(), "GB/s"));
        assert_eq!(rate(7.9e9, Unit::MBps), ("988".into(), "MB/s"));
    }

    #[test]
    fn rate_invalid() {
        assert_eq!(rate(f64::NAN, Unit::Mbps), (PLACEHOLDER.into(), "Mbps"));
        assert_eq!(rate(-1.0, Unit::MBps), (PLACEHOLDER.into(), "MB/s"));
    }

    #[test]
    fn ms_and_percent() {
        assert_eq!(format_ms(0.421), ("0.42".into(), "ms"));
        assert_eq!(format_ms(2.513), ("2.51".into(), "ms"));
        assert_eq!(format_ms(12.34), ("12.3".into(), "ms"));
        assert_eq!(format_ms(123.4), ("123".into(), "ms"));
        assert_eq!(format_ms(f64::INFINITY), (PLACEHOLDER.into(), "ms"));
        assert_eq!(format_percent(0.0), ("0".into(), "%"));
        assert_eq!(format_percent(0.0025), ("0.25".into(), "%"));
        assert_eq!(format_percent(0.015), ("1.5".into(), "%"));
        assert_eq!(format_percent(0.0996), ("10".into(), "%"));
        assert_eq!(format_percent(0.25), ("25".into(), "%"));
        assert_eq!(format_percent(1.0), ("100".into(), "%"));
        assert_eq!(format_percent(f64::NAN), (PLACEHOLDER.into(), "%"));
    }

    #[test]
    fn unit_helpers_and_serde() {
        assert_eq!(Unit::default(), Unit::Mbps);
        assert_eq!(Unit::Mbps.toggled(), Unit::MBps);
        assert_eq!(Unit::MBps.from_bps(8e6), 1.0);
        assert_eq!(serde_json::to_string(&Unit::MBps).unwrap(), "\"MB/s\"");
        assert_eq!(
            serde_json::from_str::<Unit>("\"Mbps\"").unwrap(),
            Unit::Mbps
        );
    }
}
