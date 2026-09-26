//! `domain` -- the Soulous domain as Solum understands it: the five sources, a fact, exam importance, the derived status, and the suggestions generated from it.
//!
//! Part of the `soulous` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::*;

/// The five sources Soulous currently exposes through its authenticated REST
/// controllers. Kept separate from Solum's memory layers on purpose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SoulousKind {
    Course,
    Exam,
    Task,
    Checkin,
    Focus,
}

impl SoulousKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SoulousKind::Course => "course",
            SoulousKind::Exam => "exam",
            SoulousKind::Task => "task",
            SoulousKind::Checkin => "checkin",
            SoulousKind::Focus => "focus",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SoulousKind::Course => "课表",
            SoulousKind::Exam => "考试",
            SoulousKind::Task => "学习任务",
            SoulousKind::Checkin => "打卡",
            SoulousKind::Focus => "专注",
        }
    }
}

impl std::str::FromStr for SoulousKind {
    type Err = CoreError;

    fn from_str(s: &str) -> Result<Self> {
        match s {
            "course" => Ok(SoulousKind::Course),
            "exam" => Ok(SoulousKind::Exam),
            "task" => Ok(SoulousKind::Task),
            "checkin" => Ok(SoulousKind::Checkin),
            "focus" => Ok(SoulousKind::Focus),
            _ => Err(CoreError::Invalid(format!("unknown Soulous kind: {s}"))),
        }
    }
}

/// A normalized, read-only Soulous record. `payload_json` preserves the
/// controller DTO losslessly for local inspection while the selected columns
/// support deterministic local rules and summaries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SoulousFact {
    pub id: Option<i64>,
    pub external_id: String,
    pub kind: SoulousKind,
    pub title: String,
    pub occurs_at: Option<NaiveDateTime>,
    pub ends_at: Option<NaiveDateTime>,
    pub payload_json: String,
    pub source: String,
    pub imported_at: NaiveDateTime,
}

impl SoulousFact {
    /// Stable source identity. Unlike user-authored Solum rows this is derived
    /// from Soulous's own primary key, so two Solum devices converge on one row.
    pub fn guid(&self) -> String {
        format!("soulous:{}:{}", self.kind.as_str(), self.external_id)
    }

    pub(super) fn payload(&self) -> Value {
        serde_json::from_str(&self.payload_json).unwrap_or(Value::Null)
    }
}

/// A classifier output over an actual imported exam. It does not create a Solum
/// event or notification; it simply gives F10/the UI the same canonical rule
/// decision that Solum-authored exam events receive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SoulousExamImportance {
    pub external_id: String,
    pub title: String,
    pub occurs_at: NaiveDateTime,
    pub rule: ImportanceRule,
}

pub fn classify_exams(
    facts: &[SoulousFact],
    rules: &RuleTable,
    now: NaiveDateTime,
) -> Vec<SoulousExamImportance> {
    let mut exams: Vec<_> = facts
        .iter()
        .filter(|f| f.kind == SoulousKind::Exam)
        .filter_map(|f| {
            let occurs_at = f.occurs_at?;
            (occurs_at >= now).then(|| SoulousExamImportance {
                external_id: f.external_id.clone(),
                title: f.title.clone(),
                occurs_at,
                rule: rules.rule(crate::model::EventKind::Exam),
            })
        })
        .collect();
    exams.sort_by_key(|e| e.occurs_at);
    exams
}

/// Read-only status for CLI/UI. `last_success_at` comes solely from the local
/// cached snapshot; no network call is made to render it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SoulousStatus {
    pub total: usize,
    pub courses: usize,
    pub exams: usize,
    pub tasks: usize,
    pub checkins: usize,
    pub focus_sessions: usize,
    pub last_success_at: Option<NaiveDateTime>,
    pub upcoming_exams: Vec<SoulousExamImportance>,
}

pub fn build_status(facts: &[SoulousFact], rules: &RuleTable, now: NaiveDateTime) -> SoulousStatus {
    let count = |kind| facts.iter().filter(|f| f.kind == kind).count();
    SoulousStatus {
        total: facts.len(),
        courses: count(SoulousKind::Course),
        exams: count(SoulousKind::Exam),
        tasks: count(SoulousKind::Task),
        checkins: count(SoulousKind::Checkin),
        focus_sessions: count(SoulousKind::Focus),
        last_success_at: facts.iter().map(|f| f.imported_at).max(),
        upcoming_exams: classify_exams(facts, rules, now),
    }
}

/// F10 material sourced from Soulous. It intentionally produces only local
/// suggestions, not `events`/notifications: a read-only source may inform Solum
/// but must not take over the reminder trigger chain.
pub fn generate_suggestions(
    facts: &[SoulousFact],
    now: NaiveDateTime,
    horizon_days: i64,
) -> Vec<Suggestion> {
    let until = now + chrono::Duration::days(horizon_days);
    let mut out = Vec::new();
    for fact in facts {
        let Some(when) = fact.occurs_at else { continue };
        if when <= now || when > until {
            continue;
        }
        let source = format!("soulous:{}:{}", fact.kind.as_str(), fact.external_id);
        match fact.kind {
            SoulousKind::Exam => out.push(Suggestion {
                id: None,
                created_at: now,
                kind: SuggestionKind::ExamPrep,
                text: format!(
                    "Soulous 课表显示「{}」将在 {} 考试，建议从今天开始安排复习块。",
                    fact.title,
                    fact.occurs_at.expect("checked above").format("%m-%d %H:%M")
                ),
                dedup_key: format!("exam_prep:{source}"),
                source: Some(source),
                status: SuggestionStatus::Pending,
            }),
            SoulousKind::Task if task_is_open(fact) => out.push(Suggestion {
                id: None,
                created_at: now,
                kind: SuggestionKind::DeadlineCrunch,
                text: format!(
                    "Soulous 任务「{}」截止到 {}，建议先拆出一个可完成的小步骤。",
                    fact.title,
                    fact.occurs_at.expect("checked above").format("%m-%d")
                ),
                dedup_key: format!("deadline_crunch:{source}"),
                source: Some(source),
                status: SuggestionStatus::Pending,
            }),
            _ => {}
        }
    }
    out
}

pub(super) fn task_is_open(fact: &SoulousFact) -> bool {
    !matches!(
        fact.payload()
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        "DONE" | "COMPLETED" | "CLOSED" | "CANCELLED"
    )
}
