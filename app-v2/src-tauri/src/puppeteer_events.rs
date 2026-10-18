//! Runtime-only Dance diagnostics. These never enter room history or a model prompt.
use super::{agent_id, auto_route, Strategy};
use serde::Serialize;
use std::collections::VecDeque;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Agent {
    pub agent_id: String,
    pub name: String,
}
#[derive(Clone, Debug, Serialize)]
pub(super) struct Candidate {
    #[serde(flatten)]
    pub agent: Agent,
    pub probability: f64,
}
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Outcome {
    Selected,
    Sentinel,
    Direct,
}
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Reason {
    BelowThreshold,
    EmptyText,
    NoCandidates,
    TargetLeft,
    OnlySeat,
    OnlyCandidate,
}
#[derive(Clone, Debug, Serialize)]
pub(super) struct Selection {
    pub outcome: Outcome,
    pub target: Agent,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub candidate: Option<Candidate>,
    pub threshold: f64,
    pub strategy: Strategy,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<Reason>,
}
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(super) enum Data {
    Selection(Selection),
    JevError { code: String },
    DanceError { code: String },
}
impl Data {
    pub fn error(code: &str) -> Option<Self> {
        Some(match code {
            "jev_auth" | "jev_timeout" | "jev_unreachable" | "jev_key_required"
            | "jev_key_rejected" => Self::JevError { code: code.into() },
            "dance_sentinel_required"
            | "dance_settings_changed"
            | "dance_record_failed"
            | "dance_disabled"
            | "dance_settings_invalid"
            | "dance_settings_read_failed"
            | "dance_roster_unavailable" => Self::DanceError { code: code.into() },
            // Invalid requests and the concurrency guard are not routing attempts.
            _ => return None,
        })
    }
    fn valid(&self) -> bool {
        let agent = |a: &Agent| {
            agent_id(&a.agent_id) && a.name.chars().count() <= 64 && !a.name.contains('\0')
        };
        match self {
            Self::Selection(s) => {
                agent(&s.target)
                    && auto_route::valid_threshold(s.threshold)
                    && s.candidate.as_ref().is_none_or(|c| {
                        agent(&c.agent)
                            && c.probability.is_finite()
                            && (0.0..=1.0).contains(&c.probability)
                    })
                    && match s.outcome {
                        Outcome::Selected => {
                            s.reason.is_none()
                                && s.candidate.as_ref().is_some_and(|c| {
                                    c.agent.agent_id == s.target.agent_id
                                        && c.probability > s.threshold
                                })
                        }
                        Outcome::Sentinel => match s.reason {
                            Some(Reason::BelowThreshold) => s
                                .candidate
                                .as_ref()
                                .is_some_and(|c| c.probability <= s.threshold),
                            Some(Reason::EmptyText | Reason::NoCandidates) => s.candidate.is_none(),
                            Some(Reason::TargetLeft) => true,
                            _ => false,
                        },
                        Outcome::Direct => {
                            s.candidate.is_none()
                                && matches!(
                                    s.reason,
                                    Some(Reason::OnlySeat | Reason::OnlyCandidate)
                                )
                        }
                    }
            }
            Self::JevError { code } => matches!(Self::error(code), Some(Self::JevError { .. })),
            Self::DanceError { code } => matches!(Self::error(code), Some(Self::DanceError { .. })),
        }
    }
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Event {
    pub id: u64,
    pub occurred_at: String,
    #[serde(flatten)]
    pub(super) data: Data,
}
#[derive(Default)]
pub(super) struct Log {
    next_id: u64,
    events: VecDeque<Event>,
}
impl Log {
    pub fn record(&mut self, data: Data) -> Option<Event> {
        if !data.valid() {
            return None;
        }
        self.next_id += 1;
        let event = Event {
            id: self.next_id,
            occurred_at: chrono::Utc::now().to_rfc3339(),
            data,
        };
        self.events.push_front(event.clone());
        self.events.truncate(15);
        Some(event)
    }
    pub fn snapshot(&self) -> Vec<Event> {
        self.events.iter().cloned().collect()
    }
}
