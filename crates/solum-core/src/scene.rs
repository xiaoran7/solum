//! Scene detection v1 (F13, ARCHITECTURE.md §6 Phase 7 D5) — a pure function,
//! not a state machine, and deliberately only two non-normal scenes:
//!
//! - `Sleeping`: a stored sleep session covers `now`, or it's 23:00–07:00.
//!   Proactive check-ins stay silent (event reminders still fire — F2 trumps
//!   scene politeness; an alarm you asked for must ring).
//! - `InEvent`: `now` falls inside a scheduled event (its `end`, or start+1h
//!   when open-ended). Check-ins are deferred.
//!
//! No geolocation, no app-usage spying — schedule and sleep data the user
//! already gave us are the only inputs.

use chrono::{Duration, NaiveDateTime, Timelike};

use crate::model::Event;
use crate::wearable::{HealthMetric, HealthSample};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scene {
    Sleeping,
    InEvent,
    Normal,
}

impl Scene {
    pub fn label(&self) -> &'static str {
        match self {
            Scene::Sleeping => "睡眠中",
            Scene::InEvent => "日程进行中",
            Scene::Normal => "常规",
        }
    }
}

/// Default assumed duration for events without an explicit end.
const DEFAULT_EVENT_MINUTES: i64 = 60;

/// Classify `now`. `events` may be the full table; `sleep_samples` are the
/// user's sleep sessions (only `HealthMetric::Sleep` rows are consulted).
pub fn scene(now: NaiveDateTime, events: &[Event], sleep_samples: &[HealthSample]) -> Scene {
    // A recorded sleep session covering `now` wins over the clock heuristic.
    let in_recorded_sleep = sleep_samples
        .iter()
        .filter(|s| s.kind == HealthMetric::Sleep)
        .any(|s| s.start <= now && now < s.end);
    if in_recorded_sleep || now.hour() >= 23 || now.hour() < 7 {
        return Scene::Sleeping;
    }
    let in_event = events.iter().any(|e| {
        let end = e
            .end
            .unwrap_or(e.start + Duration::minutes(DEFAULT_EVENT_MINUTES));
        e.start <= now && now < end
    });
    if in_event {
        return Scene::InEvent;
    }
    Scene::Normal
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::EventKind;
    use chrono::NaiveDate;

    fn dt(d: u32, h: u32, mi: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 7, d)
            .unwrap()
            .and_hms_opt(h, mi, 0)
            .unwrap()
    }

    #[test]
    fn night_hours_are_sleeping() {
        assert_eq!(scene(dt(6, 23, 30), &[], &[]), Scene::Sleeping);
        assert_eq!(scene(dt(6, 3, 0), &[], &[]), Scene::Sleeping);
        assert_eq!(scene(dt(6, 7, 0), &[], &[]), Scene::Normal);
    }

    #[test]
    fn recorded_sleep_session_wins_during_day() {
        let nap = HealthSample::new(HealthMetric::Sleep, dt(6, 13, 0), dt(6, 14, 0), 60.0, "hc");
        assert_eq!(
            scene(dt(6, 13, 30), &[], std::slice::from_ref(&nap)),
            Scene::Sleeping
        );
        assert_eq!(scene(dt(6, 14, 30), &[], &[nap]), Scene::Normal);
    }

    #[test]
    fn inside_event_defers() {
        let mut ev = Event::new(
            "评审会",
            EventKind::Meeting,
            dt(6, 14, 0),
            "raw",
            dt(6, 9, 0),
        );
        assert_eq!(
            scene(dt(6, 14, 30), std::slice::from_ref(&ev), &[]),
            Scene::InEvent
        );
        // Open-ended events assume one hour.
        assert_eq!(
            scene(dt(6, 15, 30), std::slice::from_ref(&ev), &[]),
            Scene::Normal
        );
        // Explicit end is honored.
        ev.end = Some(dt(6, 16, 0));
        assert_eq!(scene(dt(6, 15, 30), &[ev], &[]), Scene::InEvent);
    }
}
