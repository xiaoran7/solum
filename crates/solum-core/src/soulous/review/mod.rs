//! `review` -- the local-only F14 review material handed to the cloud rewrite, with no detail from those records in the numeric core.
//!
//! Part of the `soulous` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::domain::task_is_open;
use super::*;

/// Local-only F14 material. No details from these records are included in the
/// cloud rewrite's numeric core (review.rs appends this section afterwards).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewMaterial {
    pub courses: usize,
    pub exams: usize,
    pub open_tasks: usize,
    pub checkin_days: usize,
    pub focus_minutes: i64,
}

pub fn review_material(
    facts: &[SoulousFact],
    from: NaiveDateTime,
    to: NaiveDateTime,
) -> ReviewMaterial {
    let courses = facts
        .iter()
        .filter(|f| f.kind == SoulousKind::Course)
        .count();
    let exams = facts.iter().filter(|f| f.kind == SoulousKind::Exam).count();
    let open_tasks = facts
        .iter()
        .filter(|f| f.kind == SoulousKind::Task && task_is_open(f))
        .count();
    let checkin_days = facts
        .iter()
        .filter(|f| f.kind == SoulousKind::Checkin)
        .filter(|f| f.occurs_at.is_some_and(|t| t >= from && t <= to))
        .filter(|f| {
            f.payload()
                .get("checkedInToday")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        })
        .count();
    let focus_minutes = facts
        .iter()
        .filter(|f| f.kind == SoulousKind::Focus)
        .filter(|f| f.occurs_at.is_some_and(|t| t >= from && t <= to))
        .map(|f| {
            f.payload()
                .get("elapsedSeconds")
                .and_then(Value::as_i64)
                .unwrap_or(0)
                / 60
        })
        .sum();
    ReviewMaterial {
        courses,
        exams,
        open_tasks,
        checkin_days,
        focus_minutes,
    }
}
