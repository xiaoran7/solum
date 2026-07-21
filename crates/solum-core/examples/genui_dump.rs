//! Dev tool: print the four offline GenUI template envelopes as JSON, so
//! frontend work (and manual protocol checks) can use byte-accurate samples.
//! Run: `cargo run -p solum-core --example genui_dump`

use chrono::NaiveDate;
use solum_core::genui;
use solum_core::model::{Event, EventKind, Notification, NotificationStatus};
use solum_core::persona::PersonaDraft;
use solum_core::suggest::{Suggestion, SuggestionKind, SuggestionStatus};

fn main() {
    let now = NaiveDate::from_ymd_opt(2026, 7, 15)
        .unwrap()
        .and_hms_opt(15, 0, 0)
        .unwrap();

    let mut ev = Event::new("开会", EventKind::Meeting, now, "raw", now);
    ev.id = Some(7);
    ev.location = Some("会议室".into());
    let n = Notification {
        id: Some(42),
        event_id: 7,
        fire_at: now,
        lead_label: "30m".into(),
        channels: vec![],
        status: NotificationStatus::Pending,
        created_at: now,
        fired_at: None,
    };
    println!("--- event_ingested ---");
    println!(
        "{}",
        serde_json::to_string_pretty(&genui::event_ingested(&ev, &[n])).unwrap()
    );

    println!("--- checkin_prompt ---");
    println!(
        "{}",
        serde_json::to_string_pretty(&genui::checkin_prompt("现在在做什么？")).unwrap()
    );

    let s = Suggestion {
        id: Some(5),
        created_at: now,
        kind: SuggestionKind::ExamPrep,
        text: "「期末考试」还有 2 天，建议开始复习".into(),
        dedup_key: "exam_prep:1".into(),
        source: None,
        status: SuggestionStatus::Pending,
    };
    println!("--- suggestions_prompt ---");
    println!(
        "{}",
        serde_json::to_string_pretty(&genui::suggestions_prompt(&[s]).unwrap()).unwrap()
    );

    let draft = PersonaDraft {
        nickname: None,
        tone: "轻松直接，爱用语气词".into(),
        catchphrases: vec!["稳了".into(), "哈哈".into()],
        style_notes: Some("句子偏短".into()),
    };
    println!("--- persona_draft_form ---");
    println!(
        "{}",
        serde_json::to_string_pretty(&genui::persona_draft_form(&draft, Some("从聊天记录导入")))
            .unwrap()
    );
}
