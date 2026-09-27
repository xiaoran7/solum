//! `tests` -- undefined
//!
//! Part of the `profile_generation` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::*;

use std::sync::atomic::{AtomicU64, Ordering};

static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct TestRoot(PathBuf);

impl TestRoot {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "solum-generation-{label}-{}-{}",
            std::process::id(),
            TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        assert!(path.starts_with(std::env::temp_dir()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestRoot {
    fn drop(&mut self) {
        if self.0.starts_with(std::env::temp_dir())
            && self
                .0
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("solum-generation-"))
        {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

fn prepare(root: &Path, marker: &str) -> StagedProfileGeneration {
    let staged = begin_profile_generation(root).unwrap();
    {
        let db = staged.path().join(SQLITE_FILE);
        let store = crate::store::Store::open(db.to_str().unwrap()).unwrap();
        store
            .insert_raw_input(marker, "fixture", chrono::NaiveDateTime::default())
            .unwrap();
        store.checkpoint_for_profile_generation().unwrap();
    }
    std::fs::write(staged.path().join("platform-ready.json"), marker).unwrap();
    staged
}

#[test]
fn missing_pointer_keeps_the_legacy_profile_active() {
    let root = TestRoot::new("legacy");
    let active = resolve_active_data_dir(root.path()).unwrap();
    assert_eq!(active.path(), root.path());
    assert_eq!(active.generation_id(), None);
}

#[test]
fn sealed_generation_publishes_once_and_resolves_after_restart() {
    let root = TestRoot::new("publish");
    let sealed = seal_profile_generation(prepare(root.path(), "first")).unwrap();
    let id = sealed.id().to_owned();
    let published = publish_profile_generation(&sealed).unwrap();
    assert_eq!(published.generation_id(), Some(id.as_str()));
    assert!(published.path().join(SQLITE_FILE).is_file());

    let restarted = resolve_active_data_dir(root.path()).unwrap();
    assert_eq!(restarted, published);
    // Retrying after the live SQLite has changed must use the pointer as
    // the commit record, not compare it with the pre-publication hashes.
    let store =
        crate::store::Store::open(restarted.path().join(SQLITE_FILE).to_str().unwrap()).unwrap();
    store
        .insert_raw_input("after publish", "fixture", chrono::NaiveDateTime::default())
        .unwrap();
    drop(store);
    assert_eq!(publish_profile_generation(&sealed).unwrap(), published);
}

#[test]
fn incomplete_staging_and_unreferenced_final_directory_are_inert() {
    let root = TestRoot::new("orphan");
    let staged = prepare(root.path(), "candidate");
    assert_eq!(
        resolve_active_data_dir(root.path())
            .unwrap()
            .generation_id(),
        None
    );
    let sealed = seal_profile_generation(staged).unwrap();
    let final_path = root.path().join(GENERATIONS_DIR).join(sealed.id());
    std::fs::rename(root.path().join(STAGING_DIR).join(sealed.id()), &final_path).unwrap();
    assert_eq!(
        resolve_active_data_dir(root.path())
            .unwrap()
            .generation_id(),
        None
    );

    let resumed = resume_sealed_profile_generation(root.path(), sealed.id()).unwrap();
    let active = publish_profile_generation(&resumed).unwrap();
    assert_eq!(active.generation_id(), Some(sealed.id()));
}

#[test]
fn a_failed_second_publish_leaves_the_first_pointer_unchanged() {
    let root = TestRoot::new("rollback");
    let first = seal_profile_generation(prepare(root.path(), "first")).unwrap();
    let first_active = publish_profile_generation(&first).unwrap();

    let second = seal_profile_generation(prepare(root.path(), "second")).unwrap();
    std::fs::write(
        root.path()
            .join(STAGING_DIR)
            .join(second.id())
            .join("platform-ready.json"),
        "tampered after seal",
    )
    .unwrap();
    assert!(publish_profile_generation(&second).is_err());
    assert_eq!(resolve_active_data_dir(root.path()).unwrap(), first_active);
}

#[test]
fn damaged_pointer_fails_closed_instead_of_opening_legacy() {
    let root = TestRoot::new("pointer");
    std::fs::write(root.path().join(SQLITE_FILE), "legacy bytes").unwrap();
    std::fs::write(root.path().join(POINTER_FILE), "{\"format\":\"wrong\"}").unwrap();
    assert!(resolve_active_data_dir(root.path()).is_err());

    std::fs::write(
        root.path().join(POINTER_FILE),
        r#"{"format":"solum-profile-generation","version":1,"generation":"../escape"}"#,
    )
    .unwrap();
    assert!(resolve_active_data_dir(root.path()).is_err());
}

#[test]
fn wal_sidecars_and_post_seal_changes_are_rejected() {
    let root = TestRoot::new("integrity");
    let staged = prepare(root.path(), "wal");
    std::fs::write(staged.path().join(format!("{SQLITE_FILE}-wal")), "pending").unwrap();
    assert!(seal_profile_generation(staged).is_err());

    let invalid = begin_profile_generation(root.path()).unwrap();
    std::fs::write(invalid.path().join(SQLITE_FILE), "not sqlite").unwrap();
    assert!(seal_profile_generation(invalid).is_err());
}

#[test]
fn profile_roots_publish_independently() {
    let alice = TestRoot::new("alice");
    let bob = TestRoot::new("bob");
    let alice_sealed = seal_profile_generation(prepare(alice.path(), "alice")).unwrap();
    let bob_sealed = seal_profile_generation(prepare(bob.path(), "bob")).unwrap();
    publish_profile_generation(&alice_sealed).unwrap();
    assert!(resolve_active_data_dir(bob.path())
        .unwrap()
        .generation_id()
        .is_none());
    publish_profile_generation(&bob_sealed).unwrap();
    assert_ne!(
        resolve_active_data_dir(alice.path())
            .unwrap()
            .generation_id(),
        resolve_active_data_dir(bob.path()).unwrap().generation_id()
    );
}

#[test]
fn generation_lock_serializes_pointer_read_and_publish_boundaries() {
    let root = TestRoot::new("lock");
    let root_path = root.path().to_path_buf();
    let (locked_tx, locked_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let holder = std::thread::spawn(move || {
        with_generation_lock(&root_path, || {
            locked_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            Ok(())
        })
        .unwrap();
    });
    locked_rx.recv().unwrap();

    let second_root = root.path().to_path_buf();
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let waiter = std::thread::spawn(move || {
        with_generation_lock(&second_root, || {
            entered_tx.send(()).unwrap();
            Ok(())
        })
        .unwrap();
    });
    assert!(entered_rx
        .recv_timeout(std::time::Duration::from_millis(100))
        .is_err());
    release_tx.send(()).unwrap();
    entered_rx
        .recv_timeout(std::time::Duration::from_secs(2))
        .unwrap();
    holder.join().unwrap();
    waiter.join().unwrap();
}
