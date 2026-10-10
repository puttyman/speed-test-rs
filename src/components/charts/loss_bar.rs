//! Packet loss as one bar: received and lost packets out of those sent.

use leptos::prelude::*;

use crate::model::result::PacketLossCounts;
use crate::model::units::format_percent;

/// Lost share as a bar width percentage, with a minimum so any loss stays
/// visible.
pub fn lost_width(c: &PacketLossCounts) -> f64 {
    if c.sent == 0 || c.lost == 0 {
        return 0.0;
    }
    (f64::from(c.lost) / f64::from(c.sent) * 100.0).clamp(1.5, 100.0)
}

#[component]
pub fn LossBar(
    #[prop(into)] counts: Signal<Option<PacketLossCounts>>,
    /// One line explaining why loss is unavailable.
    #[prop(into)]
    reason: Signal<String>,
) -> impl IntoView {
    move || match counts.get() {
        Some(c) => {
            let lost = lost_width(&c);
            let (pct, unit) = format_percent(c.ratio().unwrap_or(0.0));
            view! {
                <div class="loss">
                    <div class="loss-value">
                        <span class="num">{pct}</span>
                        <span class="unit">{unit}</span>
                        <span class="muted">" lost"</span>
                    </div>
                    <div
                        class="loss-bar"
                        role="img"
                        aria-label=format!(
                            "{} of {} packets received, {} lost",
                            c.received,
                            c.sent,
                            c.lost,
                        )
                    >
                        <span class="loss-received" style=format!("flex-basis:{:.2}%", 100.0 - lost)></span>
                        {(lost > 0.0)
                            .then(|| {
                                view! {
                                    <span class="loss-lost" style=format!("flex-basis:{lost:.2}%")></span>
                                }
                            })}
                    </div>
                    <dl class="loss-legend">
                        <div>
                            <dt>
                                <span class="swatch loss-received"></span>
                                "Received"
                            </dt>
                            <dd>{c.received}</dd>
                        </div>
                        <div>
                            <dt>
                                <span class="swatch loss-lost"></span>
                                "Lost"
                            </dt>
                            <dd>{c.lost}</dd>
                        </div>
                        <div>
                            <dt>"Sent"</dt>
                            <dd>{c.sent}</dd>
                        </div>
                    </dl>
                </div>
            }
                .into_any()
        }
        None => view! {
            <div class="loss loss-unavailable">
                <div class="loss-value">
                    <span class="num muted">"Unavailable"</span>
                </div>
                <p class="muted">{move || reason.get()}</p>
            </div>
        }
        .into_any(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widths() {
        assert_eq!(lost_width(&PacketLossCounts::new(1000, 1000)), 0.0);
        assert_eq!(lost_width(&PacketLossCounts::new(1000, 999)), 1.5);
        assert_eq!(lost_width(&PacketLossCounts::new(100, 50)), 50.0);
        assert_eq!(lost_width(&PacketLossCounts::default()), 0.0);
    }
}
