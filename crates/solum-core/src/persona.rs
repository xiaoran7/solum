//! Persona Manager v1 (ARCHITECTURE.md §3.4) — manual style settings.
//!
//! The profile is a set of *readable* features (称呼/语气/口头禅/备注), never a
//! black-box vector: the user can see exactly what their "persona" says.
//! Versioning is append-only with an active-version pointer (F15 drift
//! control): every save creates version N+1, rollback just moves the pointer,
//! and the full history stays inspectable. Chat-log import is Phase 3; v1 is
//! manual-only, so `source` is always `"manual"`.
//!
//! Privacy note: the rendered [`PersonaProfile::style_prompt`] is sent to the
//! cloud reasoner when styling replies or the review digest — it is
//! user-authored config, part of the minimal per-task context. Nothing else
//! from the store rides along.

use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};

/// User-editable style settings — the input side of a persona version.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersonaDraft {
    /// 怎么称呼用户（如「老板」）。
    #[serde(default)]
    pub nickname: Option<String>,
    /// 语气描述（如「温和、简洁，偶尔冷幽默」）。
    #[serde(default)]
    pub tone: String,
    /// 常用词/口头禅。
    #[serde(default)]
    pub catchphrases: Vec<String>,
    /// 其他风格/价值观备注（自由文本）。
    #[serde(default)]
    pub style_notes: Option<String>,
}

impl PersonaDraft {
    /// Trim every field and drop empties; error if nothing is left — an
    /// all-empty persona would silently style nothing.
    pub fn normalized(mut self) -> Result<PersonaDraft> {
        self.nickname = self
            .nickname
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        self.tone = self.tone.trim().to_string();
        self.catchphrases = self
            .catchphrases
            .into_iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        self.style_notes = self
            .style_notes
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        if self.nickname.is_none()
            && self.tone.is_empty()
            && self.catchphrases.is_empty()
            && self.style_notes.is_none()
        {
            return Err(CoreError::Invalid("人格设定不能全为空".into()));
        }
        Ok(self)
    }
}

/// A persisted, versioned persona profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersonaProfile {
    pub version: i64,
    pub created_at: NaiveDateTime,
    /// v1 is always "manual"; chat-log import (Phase 3) will add others.
    pub source: String,
    /// Why this version was created (shown in the version history).
    pub note: Option<String>,
    #[serde(flatten)]
    pub draft: PersonaDraft,
}

impl PersonaProfile {
    /// Render the persona as system-prompt lines for the cloud reasoner.
    pub fn style_prompt(&self) -> String {
        let mut lines = vec!["[人格风格设定]".to_string()];
        if let Some(n) = &self.draft.nickname {
            lines.push(format!("- 称呼用户为「{n}」。"));
        }
        if !self.draft.tone.is_empty() {
            lines.push(format!("- 语气：{}。", self.draft.tone));
        }
        if !self.draft.catchphrases.is_empty() {
            lines.push(format!(
                "- 可以自然地用这些常用词（不要每句都用）：{}。",
                self.draft.catchphrases.join("、")
            ));
        }
        if let Some(s) = &self.draft.style_notes {
            lines.push(format!("- 其他：{s}"));
        }
        lines.join("\n")
    }

    /// One-line human summary for lists (CLI / version history).
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        if let Some(n) = &self.draft.nickname {
            parts.push(format!("称呼「{n}」"));
        }
        if !self.draft.tone.is_empty() {
            parts.push(format!("语气：{}", self.draft.tone));
        }
        if !self.draft.catchphrases.is_empty() {
            parts.push(format!("口头禅：{}", self.draft.catchphrases.join("、")));
        }
        if let Some(s) = &self.draft.style_notes {
            parts.push(format!("备注：{s}"));
        }
        parts.join(" · ")
    }

    pub fn to_json(&self) -> Result<String> {
        Ok(serde_json::to_string(self)?)
    }

    pub fn from_json(s: &str) -> Result<PersonaProfile> {
        Ok(serde_json::from_str(s)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn dt() -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 7, 6)
            .unwrap()
            .and_hms_opt(10, 0, 0)
            .unwrap()
    }

    fn draft() -> PersonaDraft {
        PersonaDraft {
            nickname: Some("老板".into()),
            tone: "干练、简洁".into(),
            catchphrases: vec!["稳了".into(), "安排上".into()],
            style_notes: Some("不用敬语".into()),
        }
    }

    #[test]
    fn normalize_trims_and_rejects_empty() {
        let d = PersonaDraft {
            nickname: Some("  老板 ".into()),
            tone: " 干练 ".into(),
            catchphrases: vec!["  ".into(), "稳了".into()],
            style_notes: Some("   ".into()),
        }
        .normalized()
        .unwrap();
        assert_eq!(d.nickname.as_deref(), Some("老板"));
        assert_eq!(d.tone, "干练");
        assert_eq!(d.catchphrases, vec!["稳了".to_string()]);
        assert_eq!(d.style_notes, None);

        assert!(PersonaDraft::default().normalized().is_err());
        assert!(PersonaDraft {
            tone: "  ".into(),
            ..Default::default()
        }
        .normalized()
        .is_err());
    }

    #[test]
    fn style_prompt_renders_all_fields() {
        let p = PersonaProfile {
            version: 1,
            created_at: dt(),
            source: "manual".into(),
            note: None,
            draft: draft(),
        };
        let s = p.style_prompt();
        assert!(s.contains("称呼用户为「老板」"));
        assert!(s.contains("语气：干练、简洁"));
        assert!(s.contains("稳了、安排上"));
        assert!(s.contains("不用敬语"));
        assert!(p.summary().contains("称呼「老板」"));
    }

    #[test]
    fn json_round_trip_with_flatten() {
        let p = PersonaProfile {
            version: 3,
            created_at: dt(),
            source: "manual".into(),
            note: Some("初版".into()),
            draft: draft(),
        };
        let back = PersonaProfile::from_json(&p.to_json().unwrap()).unwrap();
        assert_eq!(back, p);
        // Flattened draft fields sit at the top level of the JSON.
        assert!(p.to_json().unwrap().contains("\"nickname\""));
    }
}
