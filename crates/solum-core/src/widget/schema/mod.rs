//! `schema` -- the schema-side checks: that a mapping's targets exist, that no two fields collide, that the view mix is within budget. The reason a mapping over the same field twice is refused is in the comment right where the check is, so the rule travels with the code that enforces it.
//!
//! Part of the `widget` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::mapping::EventFieldMapping;
use super::*;

impl WidgetSchema {
    /// Which fields an event can be copied into. `None` when the widget has no
    /// text field at all — there would be nowhere to put the title, and
    /// guessing some other field would produce nonsense rows.
    pub fn event_mapping(&self) -> Option<EventFieldMapping> {
        let title = self
            .fields
            .iter()
            .find(|f| f.field_type == WidgetFieldType::Text)?;
        let when = self
            .fields
            .iter()
            .find(|f| f.field_type == WidgetFieldType::Datetime)
            .or_else(|| {
                self.fields
                    .iter()
                    .find(|f| f.field_type == WidgetFieldType::Date)
            });
        Some(EventFieldMapping {
            title: title.name.clone(),
            when: when.map(|f| f.name.clone()),
            when_type: when.map(|f| f.field_type),
        })
    }

    /// Build a record from an event, using [`Self::event_mapping`]. Required
    /// fields the mapping cannot fill are left absent on purpose — the caller
    /// validates, and a rejection is better than a fabricated value.
    pub fn record_from_event(&self, title: &str, start: NaiveDateTime) -> Option<Value> {
        let mapping = self.event_mapping()?;
        let mut data = serde_json::Map::new();
        data.insert(mapping.title, Value::String(title.to_string()));
        if let (Some(name), Some(kind)) = (mapping.when, mapping.when_type) {
            let formatted = match kind {
                WidgetFieldType::Datetime => start.format("%Y-%m-%dT%H:%M").to_string(),
                _ => start.format("%Y-%m-%d").to_string(),
            };
            data.insert(name, Value::String(formatted));
        }
        Some(Value::Object(data))
    }

    /// Read a record back as `(title, when)` for promotion to a schedule
    /// entry. `None` when the record has no usable title.
    pub fn event_from_record(&self, data: &Value) -> Option<(String, Option<NaiveDateTime>)> {
        let mapping = self.event_mapping()?;
        let title = data.get(&mapping.title)?.as_str()?.trim().to_string();
        if title.is_empty() {
            return None;
        }
        let when = mapping.when.as_ref().and_then(|name| {
            let raw = data.get(name)?.as_str()?;
            NaiveDateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M")
                .ok()
                .or_else(|| {
                    NaiveDate::parse_from_str(raw, "%Y-%m-%d")
                        .ok()?
                        .and_hms_opt(9, 0, 0)
                })
        });
        Some((title, when))
    }
}
