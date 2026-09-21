use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use solum_core::model::MemoryLayer;
use solum_core::Orchestrator;

struct FixtureDir(PathBuf);

impl FixtureDir {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "solum-cli-confirmation-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for FixtureDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).expect("remove synthetic fixture directory");
    }
}

fn run(db: &Path, args: &[&str], input: &[u8]) -> String {
    let mut child = Command::new(env!("CARGO_BIN_EXE_solum"))
        // Startup adopts legacy files from the working directory even with --db.
        .current_dir(db.parent().unwrap())
        .arg("--db")
        .arg(db)
        .arg("--now")
        .arg("2026-07-06T10:00:00")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn solum binary");
    child.stdin.take().unwrap().write_all(input).unwrap();
    let output = child.wait_with_output().expect("wait for solum binary");
    assert!(
        output.status.success(),
        "CLI failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn event_exists(db: &Path, id: i64) -> bool {
    Orchestrator::open(db.to_str().unwrap())
        .unwrap()
        .ledger()
        .unwrap()
        .iter()
        .any(|row| row.layer == MemoryLayer::Event && row.id == id)
}

#[test]
fn forget_requires_explicit_confirmation_before_deleting() {
    let fixture = FixtureDir::new();
    let db = fixture.0.join("solum.sqlite");
    let now =
        chrono::NaiveDateTime::parse_from_str("2026-07-06T10:00:00", "%Y-%m-%dT%H:%M:%S").unwrap();
    let id = {
        let mut o = Orchestrator::open(db.to_str().unwrap()).unwrap();
        // Seed through the offline core instead of loading CLI cloud configuration.
        o.ingest("明天下午3点在会议室开会", now).unwrap();
        o.ledger()
            .unwrap()
            .iter()
            .find(|row| row.layer == MemoryLayer::Event)
            .unwrap()
            .id
    };
    let id_string = id.to_string();
    let args = ["forget", "event", id_string.as_str()];
    for answer in [b"".as_slice(), b"\n", b"no\n"] {
        let output = run(&db, &args, answer);
        assert!(
            event_exists(&db, id),
            "unconfirmed CLI invocation deleted event #{id}: {output}"
        );
        assert!(output.contains("已取消"), "EOF/no must cancel: {output}");
        let o = Orchestrator::open(db.to_str().unwrap()).unwrap();
        assert!(o.audit_log().unwrap().is_empty());
    }

    let output = run(&db, &args, b"yes\n");
    assert!(
        !event_exists(&db, id),
        "explicit yes must delete event #{id}"
    );
    assert!(output.contains("输入 yes"));
    let o = Orchestrator::open(db.to_str().unwrap()).unwrap();
    let audit = o.audit_log().unwrap();
    assert_eq!(audit.len(), 1);
    assert_eq!(audit[0].tool, "memory_forget");
    assert_eq!(audit[0].decision, "executed");
    assert!(audit[0].token_id.is_some());
}
