//! Importance classification: map an event kind to *when* and *how* to warn.
//!
//! ARCHITECTURE.md §3.1 specifies a user-editable rule table with an LLM
//! fallback for kinds the table doesn't cover. Phase 1 ships the rule table
//! (the deterministic, always-available floor); the LLM fallback plugs in via
//! [`crate::extract::Reasoner`] later.
//!
//! Design note (see docs/MISC.md): each kind carries a *list* of lead times so
//! layered reminders (e.g. exam at 3d **and** 1h) are expressible. The defaults
//! match the doc's single values exactly.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};
use crate::model::{
    custom_kind_key, custom_kind_slug_value, Channel, EventKind, CUSTOM_KIND_PREFIX,
};

/// Longest accepted custom-category slug, matching the Harmony client.
const MAX_CUSTOM_KIND_SLUG: usize = 40;
/// Longest accepted custom-category name.
const MAX_CUSTOM_KIND_LABEL: usize = 40;
/// Most extraction keywords one category may carry.
const MAX_CUSTOM_KEYWORDS: usize = 8;

/// Validate a *bare* slug (no `custom:` prefix): non-empty, within the length
/// cap, CJK ideographs plus ASCII letters/digits/`-`/`_`. Byte-identical to
/// the Harmony `customKindSlug` so the same names are accepted on both ends.
fn is_valid_custom_kind_slug(slug: &str) -> bool {
    if slug.is_empty() || slug.chars().count() > MAX_CUSTOM_KIND_SLUG {
        return false;
    }
    for c in slug.chars() {
        let han = matches!(c as u32, 0x4e00..=0x9fff);
        let ascii = c.is_ascii_alphanumeric() || c == '-' || c == '_';
        if !han && !ascii {
            return false;
        }
    }
    true
}

/// The name a user may type for a category. Same charset as the slug — the
/// slug is derived from the name, so accepting different characters here
/// would let a category exist whose key can never be typed back.
pub fn is_valid_custom_kind_label(label: &str) -> bool {
    is_valid_custom_kind_slug(label) && label.chars().count() <= MAX_CUSTOM_KIND_LABEL
}

/// Matching form for an F20 important-notification routing rule. These rules
/// are only a UX/cost hint: they never authorize an action.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationMatchKind {
    #[default]
    Substring,
    Regex,
}

/// A user-editable rule that routes matching captured notifications to the
/// immediate lane. It intentionally lives in the existing importance rule
/// table rather than introducing a second rules store (Phase 10 decision 1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationPriorityRule {
    pub id: String,
    pub pattern: String,
    #[serde(default)]
    pub package_name: Option<String>,
    #[serde(default = "default_notification_priority")]
    pub priority: u8,
    #[serde(default)]
    pub matcher: NotificationMatchKind,
}

const fn default_notification_priority() -> u8 {
    1
}

/// A single "warn me this far ahead" entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeadTime {
    /// Human label such as "3d", "30m", "1h".
    pub label: String,
    /// Lead time in minutes.
    pub minutes: i64,
}

impl LeadTime {
    pub fn new(label: impl Into<String>, minutes: i64) -> Self {
        LeadTime {
            label: label.into(),
            minutes,
        }
    }

    /// Parse a compact label like "3d", "30m", "1h", "2w", "0".
    pub fn parse(label: &str) -> Result<Self> {
        let label = label.trim();
        if label == "0" {
            return Ok(LeadTime::new("0", 0));
        }
        let (num, unit) = label.split_at(
            label
                .find(|c: char| !c.is_ascii_digit())
                .ok_or_else(|| CoreError::Invalid(format!("no unit in lead time {label:?}")))?,
        );
        let n: i64 = num
            .parse()
            .map_err(|_| CoreError::Invalid(format!("bad number in lead time {label:?}")))?;
        let minutes = match unit {
            "m" | "min" => n,
            "h" | "hr" => n * 60,
            "d" => n * 60 * 24,
            "w" => n * 60 * 24 * 7,
            other => {
                return Err(CoreError::Invalid(format!(
                    "unknown lead-time unit {other:?}"
                )))
            }
        };
        Ok(LeadTime::new(label, minutes))
    }
}

/// The importance rule for one event kind.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportanceRule {
    pub kind: EventKind,
    pub lead_times: Vec<LeadTime>,
    pub channels: Vec<Channel>,
}

/// A user-created importance category: the slug, the name to show, and the
/// keywords the extractor matches on.
///
/// `slug` is stored and written **without** the `custom:` prefix, because
/// that is what the shared document says — see the [`RuleTable`] contract note
/// and the Harmony `CustomKind`. The prefix is applied where the slug becomes
/// an [`EventKind`], not here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomKind {
    pub slug: String,
    pub label: String,
    pub keywords: Vec<String>,
}

impl CustomKind {
    /// Build from a user-typed name and a bare slug.
    pub fn new(slug: &str, label: String, keywords: Vec<String>) -> Option<Self> {
        custom_kind_slug_value(slug).map(|bare| CustomKind {
            slug: bare,
            label,
            keywords,
        })
    }

    /// The `EventKind` this category keys events by.
    pub fn kind(&self) -> EventKind {
        EventKind::Custom(self.kind_key())
    }

    fn kind_key(&self) -> String {
        custom_kind_key(&self.slug).unwrap_or_else(|| format!("{CUSTOM_KIND_PREFIX}{}", self.slug))
    }
}

/// The full, user-editable rule table.
///
/// `rules` is keyed by [`EventKind`], so built-ins and user categories share
/// one map and one [`ImportanceRule`] shape — reminder scheduling never has to
/// know who created a rule. `custom_kinds` carries the display name and the
/// extraction keywords for the `Custom` entries.
///
/// The serialized form is shared with the Harmony client
/// (`entry/src/main/ets/core/classify.ets`): `rules` keyed by the kind's wire
/// string, plus a `custom_kinds` array of `{slug, label, keywords}`. Field
/// names and value shapes must stay identical on both sides — they sync the
/// same LWW document, and a mismatch drops data silently.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleTable {
    rules: HashMap<EventKind, ImportanceRule>,
    #[serde(default)]
    custom_kinds: Vec<CustomKind>,
    #[serde(default)]
    notification_priority_rules: Vec<NotificationPriorityRule>,
}

impl RuleTable {
    /// The built-in defaults from ARCHITECTURE.md §3.1 (plus sensible entries
    /// for reminder/other, which the doc leaves to defaults).
    pub fn default_table() -> Self {
        let push = || vec![Channel::Push];
        let push_banner = || vec![Channel::Push, Channel::Banner];
        let mk = |kind: EventKind, leads: &[(&str, i64)], channels: Vec<Channel>| {
            (
                kind.clone(),
                ImportanceRule {
                    kind,
                    lead_times: leads.iter().map(|(l, m)| LeadTime::new(*l, *m)).collect(),
                    channels,
                },
            )
        };
        let rules = HashMap::from([
            mk(EventKind::Exam, &[("3d", 3 * 24 * 60)], push_banner()),
            mk(EventKind::Meeting, &[("30m", 30)], push()),
            mk(EventKind::Class, &[("30m", 30)], push()),
            mk(EventKind::Deadline, &[("1d", 24 * 60)], push_banner()),
            mk(EventKind::Reminder, &[("0", 0)], push()),
            mk(EventKind::Other, &[("30m", 30)], push()),
        ]);
        RuleTable {
            rules,
            custom_kinds: Vec::new(),
            notification_priority_rules: Vec::new(),
        }
    }

    /// Look up the rule for a kind, falling back to a safe default.
    pub fn rule(&self, kind: EventKind) -> ImportanceRule {
        self.rule_ref(&kind)
    }

    /// Borrowed spelling of [`RuleTable::rule`], for callers that only read
    /// the kind off the result and don't want to clone it.
    pub fn rule_ref(&self, kind: &EventKind) -> ImportanceRule {
        self.rules.get(kind).cloned().unwrap_or(ImportanceRule {
            kind: kind.clone(),
            lead_times: vec![LeadTime::new("30m", 30)],
            channels: vec![Channel::Push],
        })
    }

    /// Insert or replace a rule (user editing the table).
    pub fn set(&mut self, rule: ImportanceRule) {
        self.rules.insert(rule.kind.clone(), rule);
    }

    /// Whether a kind has a rule of its own (as opposed to the fallback).
    pub fn has_rule(&self, kind: &EventKind) -> bool {
        self.rules.contains_key(kind)
    }

    /// The user-created categories, sorted by slug. Built-ins are not here —
    /// callers that need the whole list prepend [`EventKind::BUILTINS`].
    pub fn custom_kinds(&self) -> Vec<&CustomKind> {
        let mut out: Vec<&CustomKind> = self.custom_kinds.iter().collect();
        out.sort_by(|a, b| a.slug.cmp(&b.slug));
        out
    }

    pub fn custom_kind(&self, slug: &str) -> Option<&CustomKind> {
        self.custom_kinds.iter().find(|c| c.slug == slug)
    }

    /// The display name of a custom category, by bare or prefixed slug.
    pub fn custom_label(&self, slug: &str) -> Option<&str> {
        let bare = custom_kind_slug_value(slug)?;
        self.custom_kinds
            .iter()
            .find(|c| c.slug == bare)
            .map(|c| c.label.as_str())
    }

    /// Add a category. Returns false when the slug is already taken or when
    /// the name can't be a category key — the caller shows the message.
    pub fn add_custom_kind(&mut self, slug: String, label: String, keywords: Vec<String>) -> bool {
        let bare = match custom_kind_slug_value(&slug) {
            Some(bare) => bare,
            None => return false,
        };
        if self.custom_kinds.iter().any(|c| c.slug == bare) {
            return false;
        }
        let mut seen: Vec<String> = Vec::new();
        let mut deduped: Vec<String> = Vec::new();
        for keyword in keywords {
            let trimmed = keyword.trim().to_string();
            if trimmed.is_empty() || seen.contains(&trimmed) {
                continue;
            }
            seen.push(trimmed.clone());
            deduped.push(trimmed);
            if deduped.len() >= MAX_CUSTOM_KEYWORDS {
                break;
            }
        }
        let custom = CustomKind {
            slug: bare,
            label,
            keywords: deduped,
        };
        // A category with no rule of its own would never show up in the
        // settings list and would never schedule a reminder, so give it the
        // same starting point a new built-in gets: 30 minutes ahead, push only.
        // The caller can edit it afterwards like any other rule.
        let kind = custom.kind();
        if !self.rules.contains_key(&kind) {
            self.rules.insert(
                kind.clone(),
                ImportanceRule {
                    kind,
                    lead_times: vec![LeadTime::new("30m", 30)],
                    channels: vec![Channel::Push],
                },
            );
        }
        self.custom_kinds.push(custom);
        true
    }

    /// Remove a category and the rule attached to it. Returns false for an
    /// unknown slug; built-ins are never removable because they have no
    /// `custom_kinds` entry to match.
    pub fn remove_custom_kind(&mut self, slug: &str) -> bool {
        let bare = match custom_kind_slug_value(slug) {
            Some(bare) if self.custom_kinds.iter().any(|c| c.slug == bare) => bare,
            _ => return false,
        };
        let key = format!("{CUSTOM_KIND_PREFIX}{bare}");
        self.custom_kinds.retain(|c| c.slug != bare);
        self.rules.retain(|k, _| k.as_str() != key);
        true
    }

    /// Every kind that has a rule: the built-ins in their fixed order, then
    /// the user's categories by sorted slug. This is the order both clients
    /// render the settings list in, so the two stay comparable.
    pub fn all_rule_kinds(&self) -> Vec<EventKind> {
        let mut out: Vec<EventKind> = EventKind::BUILTINS
            .iter()
            .filter(|k| self.rules.contains_key(*k))
            .cloned()
            .collect();
        for custom in self.custom_kinds() {
            let kind = custom.kind();
            if self.rules.contains_key(&kind) {
                out.push(kind);
            }
        }
        out
    }

    pub fn notification_priority_rules(&self) -> &[NotificationPriorityRule] {
        &self.notification_priority_rules
    }

    pub fn add_notification_priority_rule(&mut self, rule: NotificationPriorityRule) {
        if let Some(existing) = self
            .notification_priority_rules
            .iter_mut()
            .find(|existing| existing.id == rule.id)
        {
            *existing = rule;
        } else {
            self.notification_priority_rules.push(rule);
        }
    }

    pub fn remove_notification_priority_rule(&mut self, id: &str) -> bool {
        let before = self.notification_priority_rules.len();
        self.notification_priority_rules
            .retain(|rule| rule.id != id);
        self.notification_priority_rules.len() != before
    }

    pub fn to_json(&self) -> Result<String> {
        Ok(serde_json::to_string(self)?)
    }

    pub fn from_json(s: &str) -> Result<Self> {
        Ok(serde_json::from_str(s)?)
    }
}

impl Default for RuleTable {
    fn default() -> Self {
        Self::default_table()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lead_time_parsing() {
        assert_eq!(LeadTime::parse("3d").unwrap().minutes, 3 * 24 * 60);
        assert_eq!(LeadTime::parse("30m").unwrap().minutes, 30);
        assert_eq!(LeadTime::parse("1h").unwrap().minutes, 60);
        assert_eq!(LeadTime::parse("2w").unwrap().minutes, 2 * 7 * 24 * 60);
        assert_eq!(LeadTime::parse("0").unwrap().minutes, 0);
        assert!(LeadTime::parse("abc").is_err());
        assert!(LeadTime::parse("3x").is_err());
    }

    #[test]
    fn defaults_match_architecture() {
        let t = RuleTable::default_table();
        let exam = t.rule(EventKind::Exam);
        assert_eq!(exam.lead_times[0].label, "3d");
        assert_eq!(exam.channels, vec![Channel::Push, Channel::Banner]);

        let meeting = t.rule(EventKind::Meeting);
        assert_eq!(meeting.lead_times[0].minutes, 30);
        assert_eq!(meeting.channels, vec![Channel::Push]);

        let deadline = t.rule(EventKind::Deadline);
        assert_eq!(deadline.lead_times[0].label, "1d");
        assert_eq!(deadline.channels, vec![Channel::Push, Channel::Banner]);
    }

    #[test]
    fn user_can_override_and_layer() {
        let mut t = RuleTable::default_table();
        t.set(ImportanceRule {
            kind: EventKind::Exam,
            lead_times: vec![
                LeadTime::parse("3d").unwrap(),
                LeadTime::parse("1h").unwrap(),
            ],
            channels: vec![Channel::Push],
        });
        assert_eq!(t.rule(EventKind::Exam).lead_times.len(), 2);
    }

    #[test]
    fn round_trips_json() {
        let t = RuleTable::default_table();
        let restored = RuleTable::from_json(&t.to_json().unwrap()).unwrap();
        assert_eq!(t, restored);
    }

    // ---- user-created categories -------------------------------------------

    fn custom_kafka() -> EventKind {
        EventKind::Custom("custom:kafka".into())
    }

    #[test]
    fn custom_slugs_accept_cjk_and_ascii_but_reject_punctuation() {
        // The normalizer always returns the bare slug, because that is what
        // the shared document stores. The prefix belongs to the EventKind.
        assert_eq!(custom_kind_slug_value("kafka").as_deref(), Some("kafka"));
        assert_eq!(custom_kind_slug_value("Kafka").as_deref(), Some("Kafka"));
        assert_eq!(custom_kind_slug_value("面试").as_deref(), Some("面试"));
        assert_eq!(custom_kind_slug_value("a-b_c1").as_deref(), Some("a-b_c1"));
        assert_eq!(
            custom_kind_slug_value("custom:kafka").as_deref(),
            Some("kafka")
        );
        assert_eq!(
            custom_kind_key("custom:kafka").as_deref(),
            Some("custom:kafka")
        );
        assert!(
            custom_kind_key("kafka").is_none(),
            "the prefix is required here"
        );
        assert!(custom_kind_key("custom:has space").is_none());
        assert!(custom_kind_slug_value("").is_none());
        assert!(custom_kind_slug_value("has space").is_none());
        assert!(custom_kind_slug_value("a.b").is_none());
        assert!(custom_kind_slug_value(&"x".repeat(41)).is_none());
        assert!(is_valid_custom_kind_label("面试"));
        assert!(!is_valid_custom_kind_label("面试 复盘"));
    }

    #[test]
    fn a_custom_kind_round_trips_as_a_flat_string_in_both_directions() {
        // The derived serde form for a newtype variant would be
        // {"custom":"kafka"}; the Harmony client writes and reads the flat
        // "custom:kafka". If either side regresses, sync silently drops it.
        assert_eq!(
            serde_json::to_string(&custom_kafka()).unwrap(),
            "\"custom:kafka\""
        );
        assert_eq!(
            serde_json::from_str::<EventKind>("\"custom:kafka\"").unwrap(),
            custom_kafka()
        );
        assert_eq!(serde_json::to_string(&EventKind::Exam).unwrap(), "\"exam\"");
        assert!(serde_json::from_str::<EventKind>("\"nonsense\"").is_err());
        assert!(serde_json::from_str::<EventKind>("\"custom:has space\"").is_err());
        // The `custom:` prefix is required, and the slug comes back normalized,
        // so a document can never introduce a bare-named category.
        assert!(serde_json::from_str::<EventKind>("\"kafka\"").is_err());
    }

    #[test]
    fn adding_a_category_stores_a_prefixed_slug_and_its_rule() {
        let mut t = RuleTable::default_table();
        assert!(t.add_custom_kind(
            "kafka".into(),
            "Kafka".into(),
            vec!["kafka".into(), "消息队列".into()]
        ));
        let kind = custom_kafka();
        assert!(t.has_rule(&kind));
        assert_eq!(t.custom_label("kafka"), Some("Kafka"));
        assert_eq!(t.custom_label("custom:kafka"), Some("Kafka"));
        assert_eq!(t.custom_kinds().len(), 1);
        assert_eq!(
            t.custom_kinds()[0].slug,
            "kafka",
            "the stored slug is bare; the prefix belongs to the kind"
        );
        assert_eq!(t.all_rule_kinds().len(), 7, "6 built-ins plus the new one");
        assert_eq!(*t.all_rule_kinds().last().unwrap(), kind);
    }

    #[test]
    fn duplicate_and_invalid_categories_are_rejected() {
        let mut t = RuleTable::default_table();
        assert!(t.add_custom_kind("kafka".into(), "Kafka".into(), vec![]));
        assert!(
            !t.add_custom_kind("kafka".into(), "Kafka again".into(), vec![]),
            "same category twice must be refused"
        );
        assert!(!t.add_custom_kind("has space".into(), "bad".into(), vec![]));
        assert_eq!(t.custom_kinds().len(), 1);
    }

    #[test]
    fn keywords_are_deduped_capped_and_trimmed() {
        let mut t = RuleTable::default_table();
        let many: Vec<String> = (0..12).map(|i| format!(" k{i} ")).collect();
        t.add_custom_kind("k".into(), "K".into(), many);
        let keywords = &t.custom_kinds()[0].keywords;
        assert_eq!(keywords.len(), MAX_CUSTOM_KEYWORDS);
        assert_eq!(keywords[0], "k0", "trimmed on the way in");
        assert!(
            !keywords.contains(&" k1 ".to_string()),
            "trimmed, not stored raw"
        );
    }

    #[test]
    fn removing_a_category_takes_its_rule_with_it() {
        let mut t = RuleTable::default_table();
        t.add_custom_kind("kafka".into(), "Kafka".into(), vec!["kafka".into()]);
        t.set(ImportanceRule {
            kind: custom_kafka(),
            lead_times: vec![LeadTime::parse("1h").unwrap()],
            channels: vec![Channel::Push],
        });
        assert!(t.has_rule(&custom_kafka()));
        assert!(t.remove_custom_kind("kafka"));
        assert!(
            !t.has_rule(&custom_kafka()),
            "the attached rule must go too"
        );
        assert!(t.custom_kinds().is_empty());
        assert!(
            !t.remove_custom_kind("kafka"),
            "removing twice reports nothing removed"
        );
        assert!(
            !t.remove_custom_kind("exam"),
            "built-ins have no custom entry"
        );
    }

    #[test]
    fn the_table_survives_a_json_round_trip_with_categories() {
        let mut t = RuleTable::default_table();
        t.add_custom_kind(
            "kafka".into(),
            "Kafka".into(),
            vec!["kafka".into(), "消息队列".into()],
        );
        t.add_custom_kind("面试".into(), "面试".into(), vec!["面试".into()]);
        t.set(ImportanceRule {
            kind: custom_kafka(),
            lead_times: vec![LeadTime::parse("1h").unwrap()],
            channels: vec![Channel::Push],
        });
        let json = t.to_json().unwrap();
        let restored = RuleTable::from_json(&json).unwrap();
        assert_eq!(t, restored);
        assert_eq!(restored.custom_kinds().len(), 2);
        assert_eq!(restored.rule(custom_kafka()).lead_times[0].minutes, 60);
        // The rules map is keyed by the kind's wire string, including the
        // `custom:` prefix; custom_kinds stores the slug bare. Both spellings
        // are load-bearing for the Harmony client reading this document.
        let doc: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(doc["rules"].get("custom:kafka").is_some());
        assert_eq!(doc["custom_kinds"][0]["slug"], "kafka");
    }

    /// The Harmony client reads this exact document (both clients sync the
    /// same LWW rule table, so a field-name or shape drift here silently
    /// discards the user's categories on the other device). This fixture is
    /// the byte-for-byte counterpart of what the Harmony `toJson` emits for
    /// the same table — if one side changes, this test is the thing that
    /// should fail first.
    #[test]
    fn the_written_document_matches_the_shape_the_harmony_client_reads() {
        let mut t = RuleTable::default_table();
        t.add_custom_kind(
            "kafka".into(),
            "Kafka".into(),
            vec!["kafka".into(), "消息队列".into()],
        );
        t.set(ImportanceRule {
            kind: custom_kafka(),
            lead_times: vec![LeadTime::new("1h", 60)],
            channels: vec![Channel::Push],
        });
        let doc: serde_json::Value = serde_json::from_str(&t.to_json().unwrap()).unwrap();

        // `rules` is a map keyed by the wire string, values carry their own
        // `kind` — the Harmony RuleJson shape.
        let exam = &doc["rules"]["exam"];
        assert_eq!(exam["kind"], "exam");
        assert_eq!(exam["lead_times"][0]["label"], "3d");
        assert_eq!(exam["lead_times"][0]["minutes"], 4320);
        assert_eq!(exam["channels"], serde_json::json!(["push", "banner"]));

        let kafka = &doc["rules"]["custom:kafka"];
        assert_eq!(kafka["kind"], "custom:kafka");
        assert_eq!(kafka["lead_times"][0]["minutes"], 60);
        assert_eq!(kafka["channels"], serde_json::json!(["push"]));

        // `custom_kinds` is an array of {slug, label, keywords}, slug bare.
        assert_eq!(
            doc["custom_kinds"],
            serde_json::json!([
                { "slug": "kafka", "label": "Kafka", "keywords": ["kafka", "消息队列"] }
            ])
        );
    }

    /// The same document in the other direction: what the Harmony client
    /// writes must load here, including its lack of a
    /// `notification_priority_rules` key.
    #[test]
    fn a_harmony_written_document_loads_and_reloads_unchanged() {
        let harmony = r#"{"rules":{"class":{"kind":"class","lead_times":[{"label":"30m","minutes":30}],"channels":["push"]},"custom:kafka":{"kind":"custom:kafka","lead_times":[{"label":"1h","minutes":60}],"channels":["push"]},"deadline":{"kind":"deadline","lead_times":[{"label":"1d","minutes":1440}],"channels":["push","banner"]},"exam":{"kind":"exam","lead_times":[{"label":"3d","minutes":4320}],"channels":["push","banner"]},"meeting":{"kind":"meeting","lead_times":[{"label":"30m","minutes":30}],"channels":["push"]},"other":{"kind":"other","lead_times":[{"label":"30m","minutes":30}],"channels":["push"]},"reminder":{"kind":"reminder","lead_times":[{"label":"0","minutes":0}],"channels":["push"]}},"custom_kinds":[{"slug":"kafka","label":"Kafka","keywords":["kafka","消息队列"]}]}"#;
        let t = RuleTable::from_json(harmony).unwrap();
        assert_eq!(t.custom_kinds().len(), 1);
        assert_eq!(t.custom_label("kafka"), Some("Kafka"));
        assert_eq!(t.rule_ref(&custom_kafka()).lead_times[0].minutes, 60);
        let kinds = t.all_rule_kinds();
        assert_eq!(kinds.len(), 7);
        assert_eq!(*kinds.last().unwrap(), custom_kafka());
        // Nothing was dropped: every built-in in the document is still here.
        assert!(kinds.contains(&EventKind::Class));
        assert!(kinds.contains(&EventKind::Reminder));
        // Re-serializing yields the same document (the Rust-only
        // notification_priority_rules is added back as an empty array).
        let back: serde_json::Value = serde_json::from_str(&t.to_json().unwrap()).unwrap();
        assert_eq!(
            back["rules"],
            serde_json::from_str::<serde_json::Value>(harmony).unwrap()["rules"]
        );
        assert_eq!(
            back["custom_kinds"],
            serde_json::from_str::<serde_json::Value>(harmony).unwrap()["custom_kinds"]
        );
    }

    #[test]
    fn a_table_without_custom_kinds_still_loads() {
        // Documents written before categories existed have no `custom_kinds`
        // key at all; they must not fail to load.
        let doc = r#"{"rules":{"exam":{"kind":"exam","lead_times":[{"label":"3d","minutes":4320}],
            "channels":["push","banner"]}}}"#;
        let t = RuleTable::from_json(doc).unwrap();
        assert!(t.custom_kinds().is_empty());
        assert_eq!(t.rule(EventKind::Exam).lead_times[0].label, "3d");
    }

    #[test]
    fn all_rule_kinds_puts_built_ins_first_then_categories_by_slug() {
        let mut t = RuleTable::default_table();
        // add_custom_kind installs a starting rule, so both show up right away.
        t.add_custom_kind("zebra".into(), "Zebra".into(), vec![]);
        t.add_custom_kind("alpha".into(), "Alpha".into(), vec![]);
        let kinds = t.all_rule_kinds();
        assert_eq!(kinds[0], EventKind::Exam);
        assert_eq!(kinds[5], EventKind::Other);
        assert_eq!(kinds[6], EventKind::Custom("custom:alpha".into()));
        assert_eq!(
            *kinds.last().unwrap(),
            EventKind::Custom("custom:zebra".into())
        );
        assert_eq!(kinds.len(), 8, "6 built-ins plus the two categories");
        // A new category starts where a new built-in would.
        assert_eq!(t.rule_ref(&kinds[6]).lead_times[0].label, "30m");
    }
}
