//! The test length slider (SPEC §7.8): a native range input over fixed stops
//! (10 s to 5 min), used by the settings drawer (Timed test length) and by the
//! host of a group test. Arrow keys, Page Up/Down, Home and End work as on any
//! range input; the value is announced as text ("1 min 30 s").

use leptos::prelude::*;

/// The slider's stops: 10, 15, 20, 30, 45 s, then 1, 2, 3, 4 and 5 min.
pub const TIMED_STOPS_MS: [u32; 10] = [
    10_000, 15_000, 20_000, 30_000, 45_000, 60_000, 120_000, 180_000, 240_000, 300_000,
];

/// The stop closest to `ms` (ties go to the shorter one).
pub fn snap_ms(ms: u32) -> u32 {
    TIMED_STOPS_MS
        .iter()
        .copied()
        .min_by_key(|s| s.abs_diff(ms))
        .unwrap_or(TIMED_STOPS_MS[0])
}

/// The stops, plus `current` when it isn't one (a test override or a group's
/// server default such as 3 s), so the slider can show it.
pub fn stops_with(current: u32) -> Vec<u32> {
    let mut v = TIMED_STOPS_MS.to_vec();
    if current > 0 && !v.contains(&current) {
        v.push(current);
        v.sort_unstable();
    }
    v
}

/// `"30 s"`, `"1 min"`, `"1 min 30 s"`, `"2.5 s"`.
pub fn format_length(ms: u32) -> String {
    let secs = f64::from(ms) / 1000.0;
    if ms < 60_000 {
        if ms.is_multiple_of(1000) {
            format!("{} s", ms / 1000)
        } else {
            format!("{secs:.1} s")
        }
    } else {
        let total = ms / 1000;
        let (m, s) = (total / 60, total % 60);
        if s == 0 {
            format!("{m} min")
        } else {
            format!("{m} min {s} s")
        }
    }
}

/// `"Download 30 s, then upload 30 s"`.
pub fn length_summary(ms: u32) -> String {
    let l = format_length(ms);
    format!("Download {l}, then upload {l}")
}

/// Time left as shown in the hero: `"23 s left"`, `"4 min 5 s left"`.
pub fn time_left(ms: f64) -> String {
    let secs = (ms.max(0.0) / 1000.0).ceil() as u32;
    if secs < 60 {
        format!("{secs} s left")
    } else if secs.is_multiple_of(60) {
        format!("{} min left", secs / 60)
    } else {
        format!("{} min {} s left", secs / 60, secs % 60)
    }
}

#[component]
pub fn LengthSlider(
    /// Current length (ms).
    #[prop(into)]
    value: Signal<u32>,
    on_change: Callback<u32>,
    /// Id of the input (for its `<label for>`).
    id: &'static str,
    #[prop(optional, into)] disabled: Signal<bool>,
) -> impl IntoView {
    let stops = Memo::new(move |_| stops_with(value.get()));
    let index = Memo::new(move |_| {
        let v = value.get();
        stops.with(|s| s.iter().position(|x| *x == v).unwrap_or(0))
    });
    let on_input = move |ev: leptos::ev::Event| {
        if let Ok(i) = event_target_value(&ev).parse::<usize>()
            && let Some(ms) = stops.with_untracked(|s| s.get(i).copied())
        {
            on_change.run(ms);
        }
    };
    view! {
        <div class="length-slider">
            <input
                id=id
                class="range"
                type="range"
                min="0"
                step="1"
                max=move || stops.with(|s| s.len().saturating_sub(1)).to_string()
                prop:value=move || index.get().to_string()
                aria-valuetext=move || format_length(value.get())
                disabled=move || disabled.get()
                style=move || {
                    let n = stops.with(Vec::len).saturating_sub(1).max(1);
                    format!("--fill:{:.1}%", index.get() as f64 / n as f64 * 100.0)
                }
                on:input=on_input
            />
            <output class="length-value" for=id aria-hidden="true">
                {move || format_length(value.get())}
            </output>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapping() {
        assert_eq!(snap_ms(0), 10_000);
        assert_eq!(snap_ms(12_000), 10_000);
        assert_eq!(snap_ms(12_500), 10_000);
        assert_eq!(snap_ms(13_000), 15_000);
        assert_eq!(snap_ms(30_000), 30_000);
        assert_eq!(snap_ms(90_000), 60_000);
        assert_eq!(snap_ms(100_000), 120_000);
        assert_eq!(snap_ms(10_000_000), 300_000);
        for s in TIMED_STOPS_MS {
            assert_eq!(snap_ms(s), s);
        }
    }

    #[test]
    fn stops_include_odd_values() {
        assert_eq!(stops_with(30_000), TIMED_STOPS_MS.to_vec());
        let v = stops_with(3000);
        assert_eq!(v.len(), 11);
        assert_eq!(v[0], 3000);
        assert!(stops_with(50_000).contains(&50_000));
    }

    #[test]
    fn length_texts() {
        assert_eq!(format_length(10_000), "10 s");
        assert_eq!(format_length(2500), "2.5 s");
        assert_eq!(format_length(60_000), "1 min");
        assert_eq!(format_length(90_000), "1 min 30 s");
        assert_eq!(format_length(300_000), "5 min");
        assert_eq!(length_summary(30_000), "Download 30 s, then upload 30 s");
        assert_eq!(time_left(22_100.0), "23 s left");
        assert_eq!(time_left(-5.0), "0 s left");
        assert_eq!(time_left(120_000.0), "2 min left");
        assert_eq!(time_left(245_000.0), "4 min 5 s left");
    }
}
