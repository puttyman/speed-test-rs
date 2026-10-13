//! Phase stepper (SPEC §7.3): Latency → Download → Upload → Packet loss. The
//! active step fills like a progress bar, finished steps get a check and
//! steps the run doesn't include (or skipped) are dimmed.

use leptos::prelude::*;

use super::icons;
use crate::model::profile::{PhaseStatus, Step};
use crate::model::protocol::Phase;

/// The four steps shown, in order.
pub const PHASES: [(Phase, &str); 4] = [
    (Phase::Latency, "Latency"),
    (Phase::Download, "Download"),
    (Phase::Upload, "Upload"),
    (Phase::PacketLoss, "Packet loss"),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepState {
    Pending,
    Active,
    Done,
    Skipped,
}

impl StepState {
    pub fn class(self) -> &'static str {
        match self {
            StepState::Pending => "is-pending",
            StepState::Active => "is-active",
            StepState::Done => "is-done",
            StepState::Skipped => "is-skipped",
        }
    }
}

/// One step of the stepper.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimelineStep {
    pub phase: Phase,
    pub label: &'static str,
    /// Progress in `0.0..=1.0`.
    pub fill: f64,
    pub state: StepState,
}

/// Derives the stepper from the profile's steps, their statuses so far, the
/// running step and its progress (`0.0..=1.0`).
pub fn timeline(
    steps: &[Step],
    statuses: &[Option<PhaseStatus>],
    current: Option<usize>,
    current_progress: f64,
) -> Vec<TimelineStep> {
    PHASES
        .iter()
        .map(|&(phase, label)| {
            let idx: Vec<usize> = steps
                .iter()
                .enumerate()
                .filter(|(_, s)| s.phase() == phase)
                .map(|(i, _)| i)
                .collect();
            if idx.is_empty() {
                return TimelineStep {
                    phase,
                    label,
                    fill: 0.0,
                    state: StepState::Skipped,
                };
            }
            let status = |i: usize| statuses.get(i).copied().flatten();
            // Phases run in blocks (SPEC §5.3): once a later step has started, this phase is
            // over, and steps that never reported (skipped by ramp halting or the time
            // budget) count as skipped.
            let last = idx[idx.len() - 1];
            let passed = current.is_some_and(|c| c > last)
                || statuses.iter().skip(last + 1).any(Option::is_some);
            let status = |i: usize| match status(i) {
                None if passed => Some(PhaseStatus::Skipped),
                s => s,
            };
            let finished = idx.iter().filter(|&&i| status(i).is_some()).count();
            let all_skipped = idx.iter().all(|&i| status(i) == Some(PhaseStatus::Skipped));
            let running_here = current.is_some_and(|c| idx.contains(&c) && status(c).is_none());
            let partial = if running_here {
                current_progress.clamp(0.0, 1.0)
            } else {
                0.0
            };
            let fill = ((finished as f64 + partial) / idx.len() as f64).clamp(0.0, 1.0);
            let state = if all_skipped {
                StepState::Skipped
            } else if finished == idx.len() {
                StepState::Done
            } else if running_here {
                StepState::Active
            } else {
                StepState::Pending
            };
            TimelineStep {
                phase,
                label,
                fill,
                state,
            }
        })
        .collect()
}

#[component]
pub fn PhaseTimeline(#[prop(into)] steps: Signal<Vec<TimelineStep>>) -> impl IntoView {
    view! {
        <ol class="timeline" aria-label="Test phases">
            {move || {
                steps
                    .get()
                    .into_iter()
                    .map(|s| {
                        let status = match s.state {
                            StepState::Pending => "not started",
                            StepState::Active => "in progress",
                            StepState::Done => "done",
                            StepState::Skipped => "skipped",
                        };
                        view! {
                            <li
                                class=format!("timeline-step {}", s.state.class())
                                aria-current=(s.state == StepState::Active).then_some("step")
                            >
                                <div class="timeline-track" aria-hidden="true">
                                    <span
                                        class="timeline-fill"
                                        style=format!("transform:scaleX({:.3})", s.fill)
                                    ></span>
                                </div>
                                <span class="timeline-label">
                                    <span class="timeline-check" aria-hidden="true">
                                        {icons::check()}
                                    </span>
                                    {s.label}
                                    <span class="visually-hidden">{format!(", {status}")}</span>
                                </span>
                            </li>
                        }
                    })
                    .collect_view()
            }}
        </ol>
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::profile::Profile;

    fn states(t: &[TimelineStep]) -> Vec<StepState> {
        t.iter().map(|s| s.state).collect()
    }

    #[test]
    fn idle_profile_marks_missing_phases_skipped() {
        let quick = Profile::quick().steps;
        let t = timeline(&quick, &[], None, 0.0);
        use StepState::*;
        assert_eq!(states(&t), vec![Pending, Pending, Pending, Skipped]);
        assert!(t.iter().all(|s| s.fill == 0.0));
    }

    #[test]
    fn progress_fills_the_active_group() {
        let steps = Profile::quick().steps;
        // quick: latency, latency, download x3, upload x2
        let mut statuses = vec![None; steps.len()];
        statuses[0] = Some(PhaseStatus::Done);
        statuses[1] = Some(PhaseStatus::Done);
        statuses[2] = Some(PhaseStatus::Done);
        let t = timeline(&steps, &statuses, Some(3), 0.5);
        use StepState::*;
        assert_eq!(states(&t), vec![Done, Active, Pending, Skipped]);
        assert_eq!(t[0].fill, 1.0);
        // 1 of 3 download steps done + half of the current one.
        assert!((t[1].fill - 0.5).abs() < 1e-9);
    }

    #[test]
    fn passed_phase_is_complete_even_with_unreported_steps() {
        let steps = Profile::standard().steps;
        // Latency done, two downloads done; the larger downloads were skipped by ramp
        // halting and never reported. Upload (index 11) is running.
        let mut statuses = vec![None; steps.len()];
        for s in statuses.iter_mut().take(5) {
            *s = Some(PhaseStatus::Done);
        }
        let t = timeline(&steps, &statuses, Some(11), 0.0);
        use StepState::*;
        assert_eq!(states(&t), vec![Done, Done, Active, Pending]);
        assert_eq!(t[1].fill, 1.0);
        assert_eq!(t[2].fill, 0.0);
    }

    #[test]
    fn skipped_and_done() {
        let steps = Profile::standard().steps;
        let statuses: Vec<_> = steps
            .iter()
            .map(|s| {
                Some(if s.phase() == Phase::PacketLoss {
                    PhaseStatus::Skipped
                } else {
                    PhaseStatus::Done
                })
            })
            .collect();
        let t = timeline(&steps, &statuses, None, 0.0);
        use StepState::*;
        assert_eq!(states(&t), vec![Done, Done, Done, Skipped]);
    }
}
