//! Reusable UI components (SPEC §7.4). The test page composes them; the
//! history, compare and run pages can reuse tiles, charts and toggles.

pub mod charts;
pub mod devices_panel;
pub mod group_view;
pub mod hero;
pub mod icons;
pub mod info_tip;
pub mod label_picker;
pub mod length_slider;
pub mod metric_tile;
pub mod motion;
pub mod phase_timeline;
pub mod presence;
pub mod pwa_toast;
pub mod quality_card;
pub mod result_view;
pub mod segmented;
pub mod server_card;
pub mod settings_drawer;
pub mod theme_toggle;
pub mod units_toggle;

use crate::model::units::{PLACEHOLDER, Unit, format_ms, format_percent, format_rate};

/// How a metric's value is formatted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetricKind {
    /// bits per second, shown in the selected unit.
    Rate,
    /// milliseconds.
    Ms,
    /// a 0..1 ratio, shown as a percentage.
    Percent,
}

impl MetricKind {
    /// Number and unit text; `None` gives the placeholder.
    pub fn format(self, v: Option<f64>, unit: Unit) -> (String, &'static str) {
        let v = v.unwrap_or(f64::NAN);
        match self {
            MetricKind::Rate => format_rate(v, unit),
            MetricKind::Ms => format_ms(v),
            // No stray "%" next to the placeholder.
            MetricKind::Percent if !v.is_finite() => (PLACEHOLDER.to_string(), ""),
            MetricKind::Percent => format_percent(v),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metric_formatting() {
        assert_eq!(
            MetricKind::Rate.format(Some(984e6), Unit::Mbps),
            ("984".into(), "Mbps")
        );
        assert_eq!(
            MetricKind::Rate.format(Some(800e6), Unit::MBps),
            ("100".into(), "MB/s")
        );
        assert_eq!(MetricKind::Ms.format(Some(2.513), Unit::Mbps).0, "2.51");
        assert_eq!(
            MetricKind::Percent.format(None, Unit::Mbps),
            (PLACEHOLDER.into(), "")
        );
        assert_eq!(MetricKind::Percent.format(Some(0.0), Unit::Mbps).1, "%");
    }
}
