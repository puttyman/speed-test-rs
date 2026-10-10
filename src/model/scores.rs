//! Cloudflare AIM (Aggregated Internet Measurement) scores (SPEC §5.9).
//!
//! Each metric maps to points through a d3 `scaleThreshold`: a value below the
//! first threshold gets the first point value, and a value *equal* to a
//! threshold falls into the next band (bisect-right). A use case's points are
//! the sum over its inputs, floored at 0, and are classified with another
//! threshold scale.

use serde::{Deserialize, Serialize};

/// Quality class of a use case, worst to best.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Classification {
    Bad,
    Poor,
    Average,
    Good,
    Great,
}

impl Classification {
    /// All classes, worst to best (index = d3 range index).
    pub const ALL: [Classification; 5] = [
        Classification::Bad,
        Classification::Poor,
        Classification::Average,
        Classification::Good,
        Classification::Great,
    ];

    /// Lower-case name as stored in the database (`"good"`).
    pub fn as_str(self) -> &'static str {
        match self {
            Classification::Bad => "bad",
            Classification::Poor => "poor",
            Classification::Average => "average",
            Classification::Good => "good",
            Classification::Great => "great",
        }
    }
}

/// The score of one use case.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Score {
    /// Sum of metric points, floored at 0.
    pub points: i32,
    pub class: Classification,
}

/// Scores of the three AIM use cases. Each is present only when all of its
/// required inputs were measured (packet loss is optional, see [`AimInputs`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Scores {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub streaming: Option<Score>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gaming: Option<Score>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rtc: Option<Score>,
}

/// Inputs of the AIM scoring. Units: milliseconds, bits per second, and a loss
/// ratio in `0.0..=1.0`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct AimInputs {
    /// Idle latency (ms).
    pub latency_ms: Option<f64>,
    /// Idle jitter (ms).
    pub jitter_ms: Option<f64>,
    /// `max(down_loaded, up_loaded) − idle latency` (ms), see [`loaded_latency_increase`].
    pub loaded_latency_increase_ms: Option<f64>,
    /// `None` = unavailable, scored as 0 points (as Cloudflare does).
    pub packet_loss_ratio: Option<f64>,
    pub download_bps: Option<f64>,
    pub upload_bps: Option<f64>,
}

/// A threshold scale: `points.len() == thresholds.len() + 1`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ThresholdScale {
    pub thresholds: &'static [f64],
    pub points: &'static [i32],
}

impl ThresholdScale {
    /// Band index of `value` (d3 `bisectRight`). `NaN` yields `None`.
    pub fn band(&self, value: f64) -> Option<usize> {
        if value.is_nan() {
            return None;
        }
        Some(self.thresholds.iter().take_while(|&&t| t <= value).count())
    }

    /// Points for `value`.
    pub fn points(&self, value: f64) -> Option<i32> {
        self.band(value).map(|i| self.points[i])
    }
}

/// Packet loss ratio → points.
pub const PACKET_LOSS: ThresholdScale = ThresholdScale {
    thresholds: &[0.01, 0.05, 0.25, 0.5],
    points: &[10, 5, 0, -10, -20],
};
/// Idle latency (ms) → points.
pub const LATENCY: ThresholdScale = ThresholdScale {
    thresholds: &[10.0, 20.0, 50.0, 100.0, 500.0],
    points: &[20, 10, 5, 0, -10, -20],
};
/// Loaded latency increase (ms) → points.
pub const LOADED_LATENCY_INCREASE: ThresholdScale = ThresholdScale {
    thresholds: &[10.0, 20.0, 50.0, 100.0, 500.0],
    points: &[20, 10, 5, 0, -10, -20],
};
/// Jitter (ms) → points.
pub const JITTER: ThresholdScale = ThresholdScale {
    thresholds: &[10.0, 20.0, 100.0, 500.0],
    points: &[10, 5, 0, -10, -20],
};
/// Download (bps) → points.
pub const DOWNLOAD: ThresholdScale = ThresholdScale {
    thresholds: &[1e6, 1e7, 5e7, 1e8],
    points: &[0, 5, 10, 20, 30],
};
/// Upload (bps) → points.
pub const UPLOAD: ThresholdScale = ThresholdScale {
    thresholds: &[1e6, 1e7, 5e7, 1e8],
    points: &[0, 5, 10, 20, 30],
};

/// Point thresholds (bad / poor / average / good / great) per use case.
pub const STREAMING_CLASSES: [i32; 4] = [15, 20, 40, 60];
pub const GAMING_CLASSES: [i32; 4] = [5, 15, 25, 30];
pub const RTC_CLASSES: [i32; 4] = [5, 15, 25, 40];

/// Classifies a points total with a threshold scale over [`Classification::ALL`].
pub fn classify(points: i32, thresholds: &[i32; 4]) -> Classification {
    let idx = thresholds.iter().take_while(|&&t| t <= points).count();
    Classification::ALL[idx]
}

/// `max(down_loaded, up_loaded) − idle`. Uses whichever loaded value exists if
/// only one does; `None` without idle latency or without any loaded value.
pub fn loaded_latency_increase(
    idle_ms: Option<f64>,
    down_loaded_ms: Option<f64>,
    up_loaded_ms: Option<f64>,
) -> Option<f64> {
    let idle = idle_ms.filter(|x| x.is_finite())?;
    let loaded = match (
        down_loaded_ms.filter(|x| x.is_finite()),
        up_loaded_ms.filter(|x| x.is_finite()),
    ) {
        (Some(d), Some(u)) => d.max(u),
        (Some(d), None) => d,
        (None, Some(u)) => u,
        (None, None) => return None,
    };
    Some(loaded - idle)
}

fn score(parts: &[Option<i32>], thresholds: &[i32; 4]) -> Option<Score> {
    let mut sum = 0;
    for p in parts {
        sum += (*p)?;
    }
    let points = sum.max(0);
    Some(Score {
        points,
        class: classify(points, thresholds),
    })
}

/// Computes all three AIM scores.
pub fn compute(inputs: &AimInputs) -> Scores {
    let pts = |scale: &ThresholdScale, v: Option<f64>| v.and_then(|v| scale.points(v));
    let latency = pts(&LATENCY, inputs.latency_ms);
    let jitter = pts(&JITTER, inputs.jitter_ms);
    let lli = pts(&LOADED_LATENCY_INCREASE, inputs.loaded_latency_increase_ms);
    // Unavailable packet loss scores 0 points rather than voiding the score.
    let loss = Some(pts(&PACKET_LOSS, inputs.packet_loss_ratio).unwrap_or(0));
    let download = pts(&DOWNLOAD, inputs.download_bps);

    Scores {
        streaming: score(&[latency, loss, download, lli], &STREAMING_CLASSES),
        gaming: score(&[latency, loss, lli], &GAMING_CLASSES),
        rtc: score(&[latency, jitter, loss, lli], &RTC_CLASSES),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Classification::*;

    fn table(scale: &ThresholdScale, cases: &[(f64, i32)]) {
        for &(v, p) in cases {
            assert_eq!(scale.points(v), Some(p), "value {v}");
        }
    }

    #[test]
    fn packet_loss_bands() {
        table(
            &PACKET_LOSS,
            &[
                (0.0, 10),
                (0.009, 10),
                (0.01, 5),
                (0.049, 5),
                (0.05, 0),
                (0.249, 0),
                (0.25, -10),
                (0.499, -10),
                (0.5, -20),
                (1.0, -20),
            ],
        );
    }

    #[test]
    fn latency_and_loaded_increase_bands() {
        let cases = [
            (-5.0, 20),
            (0.0, 20),
            (9.99, 20),
            (10.0, 10),
            (19.9, 10),
            (20.0, 5),
            (49.9, 5),
            (50.0, 0),
            (99.9, 0),
            (100.0, -10),
            (499.0, -10),
            (500.0, -20),
            (5000.0, -20),
        ];
        table(&LATENCY, &cases);
        table(&LOADED_LATENCY_INCREASE, &cases);
    }

    #[test]
    fn jitter_bands() {
        table(
            &JITTER,
            &[
                (0.0, 10),
                (9.9, 10),
                (10.0, 5),
                (19.9, 5),
                (20.0, 0),
                (99.9, 0),
                (100.0, -10),
                (499.9, -10),
                (500.0, -20),
            ],
        );
    }

    #[test]
    fn bandwidth_bands() {
        let cases = [
            (0.0, 0),
            (999_999.0, 0),
            (1e6, 5),
            (9_999_999.0, 5),
            (1e7, 10),
            (49_999_999.0, 10),
            (5e7, 20),
            (99_999_999.0, 20),
            (1e8, 30),
            (1e10, 30),
        ];
        table(&DOWNLOAD, &cases);
        table(&UPLOAD, &cases);
    }

    #[test]
    fn nan_has_no_band() {
        assert_eq!(LATENCY.points(f64::NAN), None);
    }

    #[test]
    fn classification_boundaries() {
        let cases: &[(&[i32; 4], i32, Classification)] = &[
            (&STREAMING_CLASSES, 0, Bad),
            (&STREAMING_CLASSES, 14, Bad),
            (&STREAMING_CLASSES, 15, Poor),
            (&STREAMING_CLASSES, 19, Poor),
            (&STREAMING_CLASSES, 20, Average),
            (&STREAMING_CLASSES, 39, Average),
            (&STREAMING_CLASSES, 40, Good),
            (&STREAMING_CLASSES, 59, Good),
            (&STREAMING_CLASSES, 60, Great),
            (&STREAMING_CLASSES, 80, Great),
            (&GAMING_CLASSES, 4, Bad),
            (&GAMING_CLASSES, 5, Poor),
            (&GAMING_CLASSES, 15, Average),
            (&GAMING_CLASSES, 25, Good),
            (&GAMING_CLASSES, 29, Good),
            (&GAMING_CLASSES, 30, Great),
            (&RTC_CLASSES, 4, Bad),
            (&RTC_CLASSES, 5, Poor),
            (&RTC_CLASSES, 15, Average),
            (&RTC_CLASSES, 25, Good),
            (&RTC_CLASSES, 39, Good),
            (&RTC_CLASSES, 40, Great),
        ];
        for &(t, p, c) in cases {
            assert_eq!(classify(p, t), c, "points {p} thresholds {t:?}");
        }
    }

    fn lan() -> AimInputs {
        AimInputs {
            latency_ms: Some(2.0),
            jitter_ms: Some(0.5),
            loaded_latency_increase_ms: Some(3.0),
            packet_loss_ratio: Some(0.0),
            download_bps: Some(900e6),
            upload_bps: Some(800e6),
        }
    }

    #[test]
    fn perfect_lan_is_great_everywhere() {
        let s = compute(&lan());
        // streaming: 20 + 10 + 30 + 20 = 80
        assert_eq!(
            s.streaming,
            Some(Score {
                points: 80,
                class: Great
            })
        );
        // gaming: 20 + 10 + 20 = 50
        assert_eq!(
            s.gaming,
            Some(Score {
                points: 50,
                class: Great
            })
        );
        // rtc: 20 + 10 + 10 + 20 = 60
        assert_eq!(
            s.rtc,
            Some(Score {
                points: 60,
                class: Great
            })
        );
    }

    #[test]
    fn missing_packet_loss_scores_zero_points() {
        let s = compute(&AimInputs {
            packet_loss_ratio: None,
            ..lan()
        });
        assert_eq!(s.streaming.unwrap().points, 70);
        assert_eq!(s.gaming.unwrap().points, 40);
        assert_eq!(s.rtc.unwrap().points, 50);
    }

    #[test]
    fn sum_is_floored_at_zero() {
        let s = compute(&AimInputs {
            latency_ms: Some(800.0),
            jitter_ms: Some(600.0),
            loaded_latency_increase_ms: Some(900.0),
            packet_loss_ratio: Some(0.6),
            download_bps: Some(1e5),
            upload_bps: Some(1e5),
        });
        assert_eq!(
            s.streaming,
            Some(Score {
                points: 0,
                class: Bad
            })
        );
        assert_eq!(
            s.gaming,
            Some(Score {
                points: 0,
                class: Bad
            })
        );
        assert_eq!(
            s.rtc,
            Some(Score {
                points: 0,
                class: Bad
            })
        );
    }

    #[test]
    fn scores_need_required_inputs() {
        let s = compute(&AimInputs {
            download_bps: None,
            ..lan()
        });
        assert!(s.streaming.is_none());
        assert!(s.gaming.is_some());
        assert!(s.rtc.is_some());

        let s = compute(&AimInputs {
            jitter_ms: None,
            ..lan()
        });
        assert!(s.rtc.is_none());
        assert!(s.streaming.is_some());

        let s = compute(&AimInputs {
            loaded_latency_increase_ms: None,
            ..lan()
        });
        assert_eq!(s, Scores::default());
        assert_eq!(compute(&AimInputs::default()), Scores::default());
    }

    #[test]
    fn mid_range_example() {
        // latency 25 → 5, loss 0.02 → 5, download 20 Mbps → 10, lli 60 → 0,
        // jitter 12 → 5
        let s = compute(&AimInputs {
            latency_ms: Some(25.0),
            jitter_ms: Some(12.0),
            loaded_latency_increase_ms: Some(60.0),
            packet_loss_ratio: Some(0.02),
            download_bps: Some(20e6),
            upload_bps: Some(5e6),
        });
        assert_eq!(
            s.streaming,
            Some(Score {
                points: 20,
                class: Average
            })
        );
        assert_eq!(
            s.gaming,
            Some(Score {
                points: 10,
                class: Poor
            })
        );
        assert_eq!(
            s.rtc,
            Some(Score {
                points: 15,
                class: Average
            })
        );
    }

    #[test]
    fn loaded_latency_increase_rules() {
        assert_eq!(
            loaded_latency_increase(Some(5.0), Some(20.0), Some(30.0)),
            Some(25.0)
        );
        assert_eq!(
            loaded_latency_increase(Some(5.0), Some(20.0), None),
            Some(15.0)
        );
        assert_eq!(
            loaded_latency_increase(Some(5.0), None, Some(8.0)),
            Some(3.0)
        );
        assert_eq!(loaded_latency_increase(Some(5.0), None, None), None);
        assert_eq!(loaded_latency_increase(None, Some(5.0), Some(5.0)), None);
    }

    #[test]
    fn classification_serde_and_names() {
        assert_eq!(serde_json::to_string(&Average).unwrap(), "\"average\"");
        for c in Classification::ALL {
            let json = serde_json::to_string(&c).unwrap();
            assert_eq!(json, format!("\"{}\"", c.as_str()));
        }
        let s = Scores {
            streaming: Some(Score {
                points: 30,
                class: Average,
            }),
            gaming: None,
            rtc: None,
        };
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(json, r#"{"streaming":{"points":30,"class":"average"}}"#);
        assert_eq!(serde_json::from_str::<Scores>(&json).unwrap(), s);
    }
}
