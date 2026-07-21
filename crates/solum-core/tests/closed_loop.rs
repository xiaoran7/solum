//! Integration tests that exercise solum-core through a *real, file-backed* store,
//! closing and reopening it to prove data survives across "sessions" — the
//! local-first persistence guarantee that the in-memory unit tests can't cover.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{NaiveDate, NaiveDateTime};
use solum_core::extract::Intent;
use solum_core::model::MemoryLayer;
use solum_core::proactivity::{ProactivityDimension, ProactivityLevel};
use solum_core::{CoreError, Orchestrator};

/// A unique temp db path; cleaned up (best-effort) by [`TempDb::drop`].
struct TempDb(PathBuf);

impl TempDb {
    fn new(tag: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let mut p = std::env::temp_dir();
        p.push(format!("pa_it_{tag}_{}_{nanos}.sqlite", std::process::id()));
        TempDb(p)
    }
    fn path(&self) -> &str {
        self.0.to_str().unwrap()
    }
}

impl Drop for TempDb {
    fn drop(&mut self) {
        for suffix in ["", "-wal", "-shm", "-journal"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", self.0.display()));
        }
    }
}

fn dt(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> NaiveDateTime {
    NaiveDate::from_ymd_opt(y, mo, d)
        .unwrap()
        .and_hms_opt(h, mi, 0)
        .unwrap()
}

#[test]
fn events_and_reminders_survive_reopen() {
    let db = TempDb::new("persist");
    let now = dt(2026, 7, 6, 10, 0);

    // Session 1: ingest.
    {
        let mut o = Orchestrator::open(db.path()).unwrap();
        let out = o.ingest("明天下午3点在会议室和张伟开会", now).unwrap();
        assert_eq!(out.intent, Intent::IngestEvent);
        assert_eq!(out.notifications.len(), 1);
    }

    // Session 2: reopen — everything is still there.
    {
        let mut o = Orchestrator::open(db.path()).unwrap();
        let agenda = o.agenda(now).unwrap();
        assert_eq!(agenda.len(), 1);
        assert_eq!(agenda[0].title, "开会");
        assert_eq!(agenda[0].start, dt(2026, 7, 7, 15, 0));

        // The ledger carries raw input + event + notification.
        assert_eq!(o.ledger().unwrap().len(), 3);

        // Fire the reminder exactly once, at the right time.
        assert!(o.fire_due(now).unwrap().is_empty());
        assert_eq!(o.fire_due(dt(2026, 7, 7, 14, 30)).unwrap().len(), 1);
        assert!(o.fire_due(dt(2026, 7, 7, 14, 30)).unwrap().is_empty());
    }

    // Session 3: the "fired" state also persisted.
    {
        let mut o = Orchestrator::open(db.path()).unwrap();
        assert!(o.fire_due(dt(2026, 7, 7, 14, 30)).unwrap().is_empty());
    }
}

#[test]
fn guard_audit_trail_persists() {
    let db = TempDb::new("audit");
    let now = dt(2026, 7, 6, 10, 0);

    // Session 1: a refusal and a confirmed execution.
    {
        let mut o = Orchestrator::open(db.path()).unwrap();
        let refused = o.run_tool("demo_delete", "/x", None, now);
        assert!(matches!(refused, Err(CoreError::GuardRefused(_))));

        let pending = o.request_confirmation("demo_delete", "/x", now).unwrap();
        let token = o.confirm(&pending.id, now).unwrap();
        o.run_tool("demo_delete", "/x", Some(token), now).unwrap();
    }

    // Session 2: audit rows are still there, in order.
    {
        let o = Orchestrator::open(db.path()).unwrap();
        let audit = o.audit_log().unwrap();
        assert_eq!(audit.len(), 2);
        assert_eq!(audit[0].decision, "refused");
        assert_eq!(audit[1].decision, "executed");
        assert!(audit.iter().all(|r| r.risk == "dangerous"));
    }
}

#[test]
fn forget_cascade_persists() {
    let db = TempDb::new("forget");
    let now = dt(2026, 7, 6, 10, 0);

    let raw_id;
    {
        let mut o = Orchestrator::open(db.path()).unwrap();
        let out = o.ingest("7月20号上午九点期末考试", now).unwrap();
        raw_id = out.raw_input_id;
        assert_eq!(o.ledger().unwrap().len(), 3);
    }
    {
        let o = Orchestrator::open(db.path()).unwrap();
        // Forgetting the raw input cascades to event + notification.
        o.forget(MemoryLayer::RawInput, raw_id).unwrap();
        assert!(o.ledger().unwrap().is_empty());
    }
    {
        let o = Orchestrator::open(db.path()).unwrap();
        assert!(o.ledger().unwrap().is_empty());
    }
}

#[test]
fn config_survives_reopen() {
    let db = TempDb::new("config");
    {
        let mut o = Orchestrator::open(db.path()).unwrap();
        assert!(o.notif_cloud_enabled().unwrap());
        o.set_proactivity(
            ProactivityDimension::LifeSuggestions,
            ProactivityLevel::Butler,
        )
        .unwrap();
        o.set_notif_cloud_enabled(false).unwrap();
    }
    {
        let o = Orchestrator::open(db.path()).unwrap();
        assert_eq!(
            o.proactivity().level(ProactivityDimension::LifeSuggestions),
            ProactivityLevel::Butler
        );
        assert!(!o.notif_cloud_enabled().unwrap());
    }
}
