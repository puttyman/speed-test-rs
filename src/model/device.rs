//! Device names and default labels (SPEC §7.7).
//!
//! The UI prefers User-Agent Client Hints; these heuristics are the fallback
//! (and what the server uses for `Hello`s without device info).

use super::protocol::DeviceKind;

/// Guesses the device type and a friendly name from a User-Agent string.
///
/// Heuristics, in order: smart TVs and streaming sticks, iPad, iPhone/iPod,
/// Android (phone if the UA says `Mobile`, else tablet), ChromeOS, Windows,
/// Mac, Linux. Note that iPadOS 13+ reports a Mac UA by default, so such iPads
/// show as "Mac" here; the client can refine this with touch detection.
pub fn device_kind_and_name_from_user_agent(ua: &str) -> (DeviceKind, String) {
    let lower = ua.to_ascii_lowercase();
    let has = |needle: &str| lower.contains(needle);
    let (kind, name) = if [
        "smart-tv",
        "smarttv",
        "googletv",
        "android tv",
        "appletv",
        "apple tv",
        "tizen",
        "web0s",
        "webos",
        "netcast",
        "bravia",
        "hbbtv",
        "roku",
        "crkey",
        "aftb",
        "afts",
        "aftm",
        "aftt",
        "aftka",
    ]
    .iter()
    .any(|n| has(n))
    {
        (DeviceKind::Tv, "Smart TV")
    } else if has("ipad") {
        (DeviceKind::Tablet, "iPad")
    } else if has("iphone") {
        (DeviceKind::Phone, "iPhone")
    } else if has("ipod") {
        (DeviceKind::Phone, "iPod")
    } else if has("android") {
        if has("mobile") {
            (DeviceKind::Phone, "Android phone")
        } else {
            (DeviceKind::Tablet, "Android tablet")
        }
    } else if has("cros") {
        (DeviceKind::Laptop, "Chromebook")
    } else if has("windows phone") {
        (DeviceKind::Phone, "Windows phone")
    } else if has("windows") {
        (DeviceKind::Desktop, "Windows PC")
    } else if has("macintosh") || has("mac os x") {
        (DeviceKind::Laptop, "Mac")
    } else if has("linux") || has("x11") {
        (DeviceKind::Desktop, "Linux PC")
    } else {
        (DeviceKind::Other, "Device")
    };
    (kind, name.to_string())
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// English short month name for `month` in `1..=12`.
pub fn month_abbrev(month: u32) -> Option<&'static str> {
    MONTHS.get(month.checked_sub(1)? as usize).copied()
}

/// The generated default label `"<device> · <day> <Mon>"`, e.g.
/// `default_label("iPhone", 10, 10) == "iPhone · 10 Oct"`.
///
/// The date is passed as plain components so this stays platform-free. An
/// invalid day or month drops the date part.
pub fn default_label(device_name: &str, day: u32, month: u32) -> String {
    let name = match device_name.trim() {
        "" => "Device",
        n => n,
    };
    match month_abbrev(month) {
        Some(m) if (1..=31).contains(&day) => format!("{name} · {day} {m}"),
        _ => name.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use DeviceKind::*;

    #[test]
    fn user_agents() {
        let cases: &[(&str, DeviceKind, &str)] = &[
            (
                "Mozilla/5.0 (iPhone; CPU iPhone OS 17_5 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.5 Mobile/15E148 Safari/604.1",
                Phone,
                "iPhone",
            ),
            (
                "Mozilla/5.0 (iPad; CPU OS 16_6 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/16.6 Mobile/15E148 Safari/604.1",
                Tablet,
                "iPad",
            ),
            (
                "Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Mobile Safari/537.36",
                Phone,
                "Android phone",
            ),
            (
                "Mozilla/5.0 (Linux; Android 13; SM-X710) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36",
                Tablet,
                "Android tablet",
            ),
            (
                "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.5 Safari/605.1.15",
                Laptop,
                "Mac",
            ),
            (
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36",
                Desktop,
                "Windows PC",
            ),
            (
                "Mozilla/5.0 (X11; Linux x86_64; rv:128.0) Gecko/20100101 Firefox/128.0",
                Desktop,
                "Linux PC",
            ),
            (
                "Mozilla/5.0 (X11; CrOS x86_64 14541.0.0) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36",
                Laptop,
                "Chromebook",
            ),
            (
                "Mozilla/5.0 (SMART-TV; Linux; Tizen 6.0) AppleWebKit/538.1 (KHTML, like Gecko) Version/6.0 TV Safari/538.1",
                Tv,
                "Smart TV",
            ),
            (
                "Mozilla/5.0 (Web0S; Linux/SmartTV) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/79.0.3945.79 Safari/537.36 WebAppManager",
                Tv,
                "Smart TV",
            ),
            (
                "Mozilla/5.0 (Linux; Android 9; AFTKA Build/PS7633) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0 Mobile Safari/537.36",
                Tv,
                "Smart TV",
            ),
            (
                "Mozilla/5.0 (Linux; Android 12; BRAVIA 4K VH2 Build/STT1.211025.001) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0 Safari/537.36",
                Tv,
                "Smart TV",
            ),
            ("curl/8.5.0", Other, "Device"),
            ("", Other, "Device"),
        ];
        for &(ua, kind, name) in cases {
            assert_eq!(
                device_kind_and_name_from_user_agent(ua),
                (kind, name.to_string()),
                "{ua}"
            );
        }
    }

    #[test]
    fn labels() {
        assert_eq!(default_label("iPhone", 10, 10), "iPhone · 10 Oct");
        assert_eq!(default_label("Office Mac", 1, 1), "Office Mac · 1 Jan");
        assert_eq!(default_label("Windows PC", 31, 12), "Windows PC · 31 Dec");
        assert_eq!(default_label("  ", 5, 3), "Device · 5 Mar");
        assert_eq!(default_label("iPad", 5, 13), "iPad");
        assert_eq!(default_label("iPad", 0, 3), "iPad");
        assert_eq!(month_abbrev(0), None);
        assert_eq!(month_abbrev(6), Some("Jun"));
    }
}
