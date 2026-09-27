//! `tests` -- the schema-validation and record-validation tests — the cases where a change to what counts as a valid widget would silently start accepting one nobody reviewed.
//!
//! Part of the `widget` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::*;

use serde_json::json;

fn valid() -> WidgetDefinitionDraft {
    WidgetDefinitionDraft {
        name: "课表".into(),
        icon: "calendar".into(),
        fields: vec![
            WidgetField {
                name: "course".into(),
                label: "课程".into(),
                field_type: WidgetFieldType::Text,
                required: true,
                options: vec![],
            },
            WidgetField {
                name: "starts_at".into(),
                label: "开始时间".into(),
                field_type: WidgetFieldType::Time,
                required: true,
                options: vec![],
            },
        ],
        views: vec![
            WidgetView {
                view_type: WidgetViewType::Form,
                fields: vec!["course".into(), "starts_at".into()],
                sort_by: None,
            },
            WidgetView {
                view_type: WidgetViewType::List,
                fields: vec!["course".into(), "starts_at".into()],
                sort_by: Some("starts_at".into()),
            },
        ],
    }
}

#[test]
fn accepts_the_closed_seven_type_schema_and_time_values() {
    let d = valid();
    d.validate().unwrap();
    d.schema()
        .validate_record(&json!({"course":"数学", "starts_at":"09:00"}))
        .unwrap();
    assert!(d
        .schema()
        .validate_record(&json!({"course":"数学", "starts_at":"9:00"}))
        .is_err());
}

#[test]
fn rejects_unknown_types_and_schema_keys_as_one_definition() {
    let raw = r#"{"name":"x","icon":"doc","fields":[{"name":"x","label":"X","type":"html"}],"views":[{"type":"form","fields":["x"]},{"type":"list","fields":["x"]}]}"#;
    assert!(WidgetDefinitionDraft::parse_generated(raw).is_err());
    let raw = r#"{"name":"x","icon":"doc","fields":[{"name":"x","label":"X","type":"text","script":"no"}],"views":[{"type":"form","fields":["x"]},{"type":"list","fields":["x"]}]}"#;
    assert!(WidgetDefinitionDraft::parse_generated(raw).is_err());
}

#[test]
fn rejects_limits_and_missing_field_references() {
    let mut d = valid();
    d.fields = (0..13)
        .map(|n| WidgetField {
            name: format!("field_{n}"),
            label: format!("字段{n}"),
            field_type: WidgetFieldType::Text,
            required: false,
            options: vec![],
        })
        .collect();
    assert!(d.validate().is_err());

    let mut d = valid();
    d.views[1].fields.push("missing".into());
    assert!(d.validate().is_err());

    let mut d = valid();
    d.views = (0..5)
        .map(|_| WidgetView {
            view_type: WidgetViewType::List,
            fields: vec!["course".into()],
            sort_by: None,
        })
        .collect();
    assert!(d.validate().is_err());
}

#[test]
fn record_validation_rejects_instead_of_coercing_or_dropping() {
    let schema = valid().schema();
    assert!(schema
        .validate_record(&json!({"course":"数学", "starts_at":900}))
        .is_err());
    assert!(schema
        .validate_record(&json!({"course":"数学", "starts_at":"09:00", "extra":true}))
        .is_err());
}
