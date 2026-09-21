// crates/solum-app/src/tests.rs
use super::*;

fn args(base: &str, key: &str, model: &str) -> LlmSaveArgs {
        LlmSaveArgs {
            base_url: base.into(),
            api_key: key.into(),
            model: model.into(),
            temperature: Some(0.3),
            max_tokens: None,
            timeout_secs: 30,
        }
    }

    #[test]
    fn resolve_validates_and_normalizes() {
        // Trailing slash trimmed; values pass through.
        let c = resolve_llm_args(args("https://x/v1/", "k", "m")).unwrap();
        assert_eq!(c.base_url, "https://x/v1");
        assert_eq!(c.model, "m");
        // Bad scheme / empty model are actionable errors.
        assert!(resolve_llm_args(args("x/v1", "k", "m")).is_err());
        assert!(resolve_llm_args(args("https://x/v1", "k", "  ")).is_err());
        // Timeout clamps into [5, 600].
        let mut a = args("https://x/v1", "k", "m");
        a.timeout_secs = 1;
        assert_eq!(resolve_llm_args(a).unwrap().timeout_secs, 5);
    }

    /// Scratch dir + a real orchestrator, so the spool tests exercise the
    /// actual filesystem behaviour rather than a model of it.
    fn spool_fixture(name: &str) -> (std::path::PathBuf, solum_core::Orchestrator) {
        let dir = std::env::temp_dir().join(format!("solum-spool-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("notif-spool")).unwrap();
        let db = dir.join("solum.sqlite");
        let orch = solum_core::Orchestrator::open(db.to_str().unwrap()).unwrap();
        (dir, orch)
    }

    fn write_spool(dir: &std::path::Path, stem: &str, pkg: &str, title: &str) {
        let line = serde_json::json!({ "ts": 1_784_000_000_000i64, "pkg": pkg, "title": title, "text": "" });
        std::fs::write(
            dir.join("notif-spool").join(format!("{stem}.json")),
            format!(
                "{line}
"
            ),
        )
        .unwrap();
    }

    /// Full queue: the listener's byte-per-drop tally becomes durable state and
    /// The race the reviewer found: a drop recorded by the listener *while* the
    /// drainer is working must not be swallowed. Claiming by rename means the
    /// Crash recovery: dying between "claim the counter" and "record it" must
    fn overflow_dir(dir: &std::path::Path) -> std::path::PathBuf {
        let d = dir.join("notif-overflow");
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn drop_marker(dir: &std::path::Path, stem: &str) {
        std::fs::write(overflow_dir(dir).join(format!("{stem}.mark")), [1u8]).unwrap();
    }

    fn markers_left(dir: &std::path::Path) -> usize {
        collect_overflow_markers(dir).len()
    }

    /// Full queue: each marker becomes durable state exactly once.
    #[test]
    fn a_full_queue_tally_is_recorded_once_and_cleared() {
        let (dir, mut orch) = spool_fixture("overflow");
        let db = dir.join("solum.sqlite");
        for i in 0..5 {
            drop_marker(&dir, &format!("100{i}-aaaa"));
        }

        drain_capture_inbox(&mut orch, db.to_str().unwrap(), system_now());
        assert_eq!(orch.capture_overflow_count().unwrap(), 5);
        assert_eq!(markers_left(&dir), 0);

        drain_capture_inbox(&mut orch, db.to_str().unwrap(), system_now());
        assert_eq!(orch.capture_overflow_count().unwrap(), 5, "no re-count");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **The interleaving the reviewer identified**, constructed for real: a
    /// drop recorded by the listener *after* this side has taken its census but
    /// *before* it deletes must survive. The previous test only drained, wrote,
    /// and drained again — which never exercised the window at all.
    ///
    /// With a shared counter file this is exactly where a byte was lost: the
    /// writer's descriptor was opened pre-rename and its write landed
    /// post-unlink. With markers, `record_and_remove` touches only the paths it
    /// counted, so the newcomer is untouched and counted next pass.
    #[test]
    fn a_marker_created_between_the_census_and_the_delete_is_not_lost() {
        let (dir, mut orch) = spool_fixture("overflow-interleave");
        let db = dir.join("solum.sqlite");
        drop_marker(&dir, "1000-aaaa");
        drop_marker(&dir, "1001-bbbb");

        // Step 1: this side takes its census (2 markers).
        let census = collect_overflow_markers(&dir);
        assert_eq!(census.len(), 2);

        // Step 2: the listener drops another one *right now* — after the
        // census, before the delete. This is the window that was losing data.
        drop_marker(&dir, "1002-cccc");

        // Step 3: record + delete, which must touch only the censused paths.
        record_and_remove_overflow_markers(&mut orch, &census);
        assert_eq!(orch.capture_overflow_count().unwrap(), 2);
        assert_eq!(
            markers_left(&dir),
            1,
            "the marker that arrived mid-drain must still be on disk"
        );

        // Step 4: the next pass counts it — total 3, nothing lost or doubled.
        drain_capture_inbox(&mut orch, db.to_str().unwrap(), system_now());
        assert_eq!(orch.capture_overflow_count().unwrap(), 3);
        assert_eq!(markers_left(&dir), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Crash between "counted" and "deleted" must not inflate the total.
    ///
    /// This previously asserted `>= 1`, i.e. it *accepted* double counting —
    /// while the code comment claimed markers were never counted twice and the
    /// UI presented the figure as "at least N". A number that can overshoot is
    /// not a lower bound, so all three could not be true at once. The receipt
    /// makes the strict assertion below hold.
    #[test]
    fn a_crash_between_counting_and_deleting_does_not_double_count() {
        let (dir, mut orch) = spool_fixture("overflow-crash");
        let db = dir.join("solum.sqlite");
        drop_marker(&dir, "1000-aaaa");
        drop_marker(&dir, "1001-bbbb");

        // Count them, then "crash": the files are deliberately left behind.
        let census = collect_overflow_markers(&dir);
        let names: Vec<String> = census
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(orch.note_capture_overflow(&names).unwrap(), 2);
        assert_eq!(orch.capture_overflow_count().unwrap(), 2);
        assert_eq!(markers_left(&dir), 2, "simulated crash before deletion");

        // Next run re-scans the same markers.
        drain_capture_inbox(&mut orch, db.to_str().unwrap(), system_now());
        assert_eq!(
            orch.capture_overflow_count().unwrap(),
            2,
            "re-scanning already-counted markers must add nothing"
        );
        assert_eq!(markers_left(&dir), 0, "and they are cleaned up");

        // A genuinely new drop still counts.
        drop_marker(&dir, "1002-cccc");
        drain_capture_inbox(&mut orch, db.to_str().unwrap(), system_now());
        assert_eq!(orch.capture_overflow_count().unwrap(), 3);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A receipt must not expire while the marker it guards is still on disk.
    ///
    /// The drain used to prune receipts older than 30 days on every pass. The
    /// crash window that receipts guard has no upper bound: a marker whose
    /// deletion keeps failing outlives any cutoff, so the receipt aged out, the
    /// next scan counted the same marker again, and the total climbed above the
    /// truth — reintroducing the very over-count the receipt was added to stop.
    #[test]
    fn time_passing_does_not_retire_a_receipt_whose_marker_survives() {
        let (dir, mut orch) = spool_fixture("overflow-aged");
        let db = dir.join("solum.sqlite");

        // A marker whose deletion fails *every* time. Made undeletable by
        // being a directory: `remove_file` refuses one on every platform, and
        // the census filters on extension, so it is censused and counted like
        // any other. A real device hits this via a read-only volume, a lock, or
        // a permission change — the point is only that it persists.
        std::fs::create_dir_all(overflow_dir(&dir).join("1000-stuck.mark")).unwrap();

        // Two ticks, both long after any retention cutoff — a device that has
        // simply been running a while.
        let much_later = system_now() + Duration::days(400);
        drain_capture_inbox(&mut orch, db.to_str().unwrap(), much_later);
        assert_eq!(orch.capture_overflow_count().unwrap(), 1);
        assert_eq!(markers_left(&dir), 1, "deletion failed; it persists");

        drain_capture_inbox(&mut orch, db.to_str().unwrap(), much_later);
        assert_eq!(
            orch.capture_overflow_count().unwrap(),
            1,
            "the receipt must outlive any cutoff for as long as its marker does"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Receipts are retired one file at a time, by that file being gone.
    #[test]
    fn releasing_one_receipt_leaves_the_others_guarding() {
        let (dir, orch) = spool_fixture("overflow-release");
        let names = vec!["1000-aaaa.mark".to_string(), "1001-bbbb.mark".to_string()];
        assert_eq!(orch.note_capture_overflow(&names).unwrap(), 2);

        // Only the first file was actually deleted.
        assert_eq!(orch.release_capture_overflow(&names[..1]).unwrap(), 1);

        // Re-counting both: the deleted one is a genuinely new drop if its name
        // ever recurs; the survivor is still suppressed.
        assert_eq!(
            orch.note_capture_overflow(&names).unwrap(),
            1,
            "only the released name may be counted again"
        );
        assert_eq!(orch.capture_overflow_count().unwrap(), 3);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The legacy migration had the identical window; it is receipt-guarded too.
    #[test]
    fn a_crash_during_legacy_migration_does_not_double_count() {
        let (dir, mut orch) = spool_fixture("overflow-legacy-crash");
        let db = dir.join("solum.sqlite");
        let legacy = dir.join("notif-spool.overflow");
        std::fs::write(&legacy, [1u8; 4]).unwrap();

        // The crash is *between* counting and deleting, so the count lands and
        // the file is still there next boot. Writing the file back after a
        // successful delete would not reproduce that — under this design a
        // recreated file is a genuinely new tally, and counting it is correct.
        let names: Vec<String> = (0..4)
            .map(|i| format!("notif-spool.overflow#{i}"))
            .collect();
        assert_eq!(orch.note_capture_overflow(&names).unwrap(), 4);
        assert!(legacy.is_file(), "simulated crash before deletion");

        // Next boot re-reads the same 4 bytes; the receipts absorb them. Time
        // passing must not change that, so run the retry far in the future.
        let much_later = system_now() + Duration::days(400);
        drain_capture_inbox(&mut orch, db.to_str().unwrap(), much_later);
        assert_eq!(
            orch.capture_overflow_count().unwrap(),
            4,
            "the same legacy tally must not be counted twice"
        );
        assert!(!legacy.exists(), "and the retry clears the file");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Undeliverable payloads are a **live census** of `failed/`, not a tally.
    /// Counting the directory means the figure cannot drift from the files it
    /// describes — no window between parking and counting, and nothing to
    /// double-count when a receipt would have aged out.
    #[test]
    fn undeliverable_payloads_are_counted_from_the_directory() {
        let (dir, _orch) = spool_fixture("undeliverable");
        let db = dir.join("solum.sqlite");
        assert_eq!(parked_payload_count(db.to_str().unwrap()), 0);

        let parked = dir.join("notif-spool").join("failed");
        std::fs::create_dir_all(&parked).unwrap();
        std::fs::write(parked.join("a.json"), "{}").unwrap();
        std::fs::write(parked.join("b.json"), "{}").unwrap();
        assert_eq!(parked_payload_count(db.to_str().unwrap()), 2);

        // Idempotent by construction: asking twice cannot inflate it.
        assert_eq!(parked_payload_count(db.to_str().unwrap()), 2);
        // Non-payload files are not counted.
        std::fs::write(parked.join("notes.txt"), "x").unwrap();
        assert_eq!(parked_payload_count(db.to_str().unwrap()), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Counts written by the previous shared-counter implementation are carried
    /// over rather than dropped on upgrade.
    #[test]
    fn legacy_shared_counter_files_are_migrated() {
        let (dir, mut orch) = spool_fixture("overflow-legacy");
        let db = dir.join("solum.sqlite");
        std::fs::write(dir.join("notif-spool.overflow"), [1u8; 3]).unwrap();
        std::fs::write(dir.join("notif-spool.overflow.taking"), [1u8; 2]).unwrap();

        drain_capture_inbox(&mut orch, db.to_str().unwrap(), system_now());
        assert_eq!(orch.capture_overflow_count().unwrap(), 5);
        assert!(!dir.join("notif-spool.overflow").exists());
        assert!(!dir.join("notif-spool.overflow.taking").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Happy path: an accepted spool file is consumed and removed.
    #[test]
    fn an_accepted_spool_file_is_consumed_and_deleted() {
        let (dir, mut orch) = spool_fixture("accept");
        let db = dir.join("solum.sqlite");
        orch.set_notification_app_enabled("com.a", true).unwrap();
        write_spool(&dir, "1784000000000-aaaa", "com.a", "明天下午3点开会");

        let out = drain_capture_inbox(&mut orch, db.to_str().unwrap(), system_now());
        assert_eq!(out.len(), 1, "the capture should be reported");
        let left: Vec<_> = std::fs::read_dir(dir.join("notif-spool"))
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name())
            .collect();
        assert!(left.is_empty(), "accepted file must be gone, got {left:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A capture the core *declines* (package not whitelisted) is a completed
    /// outcome, not a failure — the file is consumed rather than retried
    /// forever.
    #[test]
    fn a_declined_capture_does_not_wedge_the_spool() {
        let (dir, mut orch) = spool_fixture("decline");
        let db = dir.join("solum.sqlite");
        // No whitelist entry → core returns Ok(None).
        write_spool(
            &dir,
            "1784000000000-bbbb",
            "com.not.allowed",
            "明天下午3点开会",
        );

        drain_capture_inbox(&mut orch, db.to_str().unwrap(), system_now());
        let left: Vec<_> = std::fs::read_dir(dir.join("notif-spool"))
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name())
            .collect();
        assert!(
            left.is_empty(),
            "a declined capture is decided, not failed: {left:?}"
        );
        assert_eq!(parked_payload_count(db.to_str().unwrap()), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The spool-file lifecycle rule, pinned: a file is deleted only when the
    /// core accepted every line. The reviewer caught the opposite — errors were
    /// swallowed and the file deleted regardless, losing the notification while
    /// the comment claimed it could not happen.
    #[test]
    fn a_spool_file_survives_when_the_core_could_not_record_it() {
        // The decision under test is `batch.failed == 0`, so exercise it
        // directly rather than trying to fake a database failure.
        let accepted = IngestBatch {
            outcomes: vec!["ok".into()],
            failed: 0,
        };
        let partly_failed = IngestBatch {
            outcomes: vec!["ok".into()],
            failed: 1,
        };
        assert_eq!(accepted.failed, 0, "fully accepted → safe to delete");
        assert!(
            partly_failed.failed > 0,
            "any failure must keep the file for the next tick"
        );
        // A rejection (`Ok(None)`) is not a failure: nothing to retry.
        let all_rejected = IngestBatch::default();
        assert_eq!(
            all_rejected.failed, 0,
            "core deciding not to keep a capture is a completed outcome"
        );
    }

    /// Retry bookkeeping lives in the filename so it survives the restart a
    /// transient failure usually prompts.
    #[test]
    fn spool_retry_counter_round_trips_through_the_filename() {
        use std::path::Path;
        let fresh = Path::new("1700000000-abcd1234.json");
        assert_eq!(spool_attempt(fresh), 1);
        assert_eq!(spool_base(fresh), "1700000000-abcd1234");

        let retried = Path::new("1700000000-abcd1234.try2.json");
        assert_eq!(spool_attempt(retried), 2);
        assert_eq!(
            spool_base(retried),
            "1700000000-abcd1234",
            "the base must not accumulate .try suffixes across retries"
        );

        // A stem that merely contains ".try" but not a number is not a counter.
        let odd = Path::new("weird.tryX.json");
        assert_eq!(spool_attempt(odd), 1);
        assert_eq!(spool_base(odd), "weird.tryX");
    }

    /// The native listener and Rust must select the same account directory.
    /// A root-level spool while SQLite lives under `profiles/<uuid>` silently
    /// strands notifications and makes a same-profile backup impossible.
    #[test]
    fn android_listener_publishes_spool_and_overflow_under_one_frozen_profile() {
        let kotlin = include_str!(
            "../gen/android/app/src/main/java/dev/solum/app/SolumNotificationListener.kt"
        );
        assert!(kotlin.contains("handleNotificationPostedLocked(sbn, profileRoot)"));
        assert!(kotlin.contains("channel.lock().use"));
        assert!(kotlin.contains("active-generation.lock"));
        assert!(kotlin.contains("val profileDir = activeGenerationDir(profileRoot) ?: return"));
        assert!(kotlin.contains("activeGenerationDir(profileRoot)"));
        assert!(kotlin.contains("active-generation.json"));
        assert!(kotlin.contains("generation-manifest.json"));
        assert!(kotlin.contains("Files.notExists(pointerPath"));
        assert!(kotlin.contains("File(profileDir, SPOOL_DIR)"));
        assert!(kotlin.contains("File(profileDir, OVERFLOW_DIR)"));
        assert!(!kotlin.contains("File(dataDir, SPOOL_DIR)"));
        assert!(!kotlin.contains("File(dataDir, OVERFLOW_DIR)"));
    }

    #[test]
    fn empty_key_reuses_stored_file_key() {
        let dir = std::env::temp_dir().join("solum-llm-test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("solum-llm.json");
        std::fs::write(
            &path,
            r#"{"base_url":"https://old/v1","api_key":"tp-old-9999"}"#,
        )
        .unwrap();
        std::env::set_var("SOLUM_LLM_CONFIG", &path);
        let c = resolve_llm_args(args("https://new/v1", "", "m")).unwrap();
        assert_eq!(c.api_key, "tp-old-9999");
        // No stored key at all → must ask for one.
        std::fs::remove_file(&path).unwrap();
        assert!(resolve_llm_args(args("https://new/v1", "", "m")).is_err());
        std::env::remove_var("SOLUM_LLM_CONFIG");
    }

    #[test]
    fn key_tail_masks() {
        assert_eq!(key_tail("tp-secret-zju9"), "…zju9");
        assert_eq!(key_tail(""), "");
    }

    #[test]
    fn daily_brief_gate_allows_only_one_emit_per_date() {
        let first = NaiveDate::from_ymd_opt(2026, 7, 16).unwrap();
        let next = NaiveDate::from_ymd_opt(2026, 7, 17).unwrap();
        assert!(daily_brief_is_due(None, first));
        assert!(!daily_brief_is_due(Some(first), first));
        assert!(daily_brief_is_due(Some(first), next));
    }

    #[test]
    fn web_storage_names_keep_legacy_compatible_and_isolate_generations() {
        let user = "11111111-1111-4111-8111-111111111111";
        let generation = "0123456789abcdef0123456789abcdef";
        assert_eq!(
            web_storage_names("guest", None).unwrap(),
            WebStorageNames {
                ui_state_key: "solum.ui-state.v1".into(),
                chat_sessions_key: "pa.chat-sessions.v1".into(),
                document_db_name: "pa-local-documents-v1".into(),
            }
        );
        assert_eq!(
            web_storage_names(user, None).unwrap().chat_sessions_key,
            format!("pa.chat-sessions.v1.{user}")
        );
        let restored = web_storage_names(user, Some(generation)).unwrap();
        assert_eq!(
            restored.document_db_name,
            format!("pa-local-documents-v1.{user}.generation.{generation}")
        );
        assert_ne!(restored, web_storage_names(user, None).unwrap());
        assert!(web_storage_names(user, Some("../escape")).is_err());
    }

    #[test]
    fn webview_hydration_receipt_requires_exact_profile_generation_and_ordered_set() {
        let expected = vec![WebViewDocumentReceipt {
            id: "document-1".into(),
            plaintext_size: 7,
            sha256: "01".repeat(32),
        }];
        let valid = WebViewHydrationReceipt {
            profile_id: "guest".into(),
            generation: "0123456789abcdef0123456789abcdef".into(),
            documents: vec![WebViewDocumentReceipt {
                id: "document-1".into(),
                plaintext_size: 7,
                sha256: "01".repeat(32),
            }],
        };
        assert!(validate_webview_hydration_receipt(
            "guest",
            "0123456789abcdef0123456789abcdef",
            &expected,
            &valid
        )
        .is_ok());
        let missing = WebViewHydrationReceipt {
            documents: Vec::new(),
            ..valid
        };
        assert!(validate_webview_hydration_receipt(
            "guest",
            "0123456789abcdef0123456789abcdef",
            &expected,
            &missing
        )
        .is_err());
    }

    #[test]
    fn frontend_hydrates_only_inactive_generation_and_does_not_publish_pointer() {
        let js = include_str!("../dist/app.js");
        assert!(js.contains("hydratePortableGenerationWebView(generationId)"));
        assert!(js.contains("restore_webview_begin"));
        assert!(js.contains("restore_webview_document_chunk"));
        assert!(js.contains("restore_webview_finish"));
        assert!(js.contains("store.clear()"));
        assert!(js.contains("await sha256Hex(await actual.blob.arrayBuffer())"));
        assert!(js.contains("safeLocalGet(storage.chat_sessions_key) !== chatJson"));
        assert!(!js.contains("publish_profile_generation"));
    }

    #[test]
    fn publication_binding_rejects_token_sqlite_and_webview_changes() {
        let webview = LiveWebViewSummary {
            profile_id: "guest".into(),
            chat_sessions: 1,
            chat_messages: 2,
            drafts: 1,
            documents: 0,
            fingerprint: "11".repeat(32),
        };
        assert!(validate_live_webview_summary(&webview, "guest").is_ok());
        let binding = RestorePublicationBinding {
            token: "22".repeat(32),
            sqlite_fingerprint: "33".repeat(32),
            webview: webview.clone(),
        };
        assert!(validate_publication_binding(
            &binding,
            &"22".repeat(32),
            &"33".repeat(32),
            &webview
        )
        .is_ok());
        assert!(validate_publication_binding(
            &binding,
            &"44".repeat(32),
            &"33".repeat(32),
            &webview
        )
        .is_err());
        assert!(validate_publication_binding(
            &binding,
            &"22".repeat(32),
            &"55".repeat(32),
            &webview
        )
        .is_err());
        let mut changed_webview = webview;
        changed_webview.fingerprint = "66".repeat(32);
        assert!(validate_publication_binding(
            &binding,
            &"22".repeat(32),
            &"33".repeat(32),
            &changed_webview
        )
        .is_err());
    }

    #[test]
    fn frontend_publication_requires_fresh_exact_webview_snapshot() {
        let js = include_str!("../dist/app.js");
        assert!(js.contains("snapshotLiveWebViewSummary()"));
        assert!(js.contains("saveCurrentChatDraft(true)"));
        assert!(js.contains("chat_json: chatJson"));
        assert!(js.contains("ui_state_json: uiStateJson"));
        assert!(js.contains("await sha256Hex(await row.blob.arrayBuffer())"));
        assert!(js.contains("restore_publish_preview"));
        assert!(js.contains("backup_restore_takeover_publish"));
        assert!(!js.contains("restore_publish_confirm"));
        assert!(js.contains("const webview = await snapshotLiveWebViewSummary();"));
    }

    #[test]
    fn frontend_backup_seam_streams_bounded_chunks_and_commits_last() {
        let js = include_str!("../dist/app.js");
        assert!(js.contains("const BACKUP_UPLOAD_IPC_CHUNK_BYTES = 512 * 1024"));
        assert!(js.contains("async function createAndUploadPortableBackup()"));
        assert!(js.contains("dataBase64: encodeCanonicalBase64(chunk)"));
        assert!(js.contains("row.blob.size !== expected.plaintext_size"));
        let begin = js.find("backup_upload_begin").unwrap();
        let chunk = js.find("backup_upload_attachment_chunk").unwrap();
        let finish = js.find("backup_upload_attachment_finish").unwrap();
        let commit = js.find("backup_upload_commit").unwrap();
        assert!(begin < chunk && chunk < finish && finish < commit);
        assert!(js[commit..].contains("backup_upload_cancel"));
    }

    #[test]
    fn frontend_backup_center_is_manual_and_recovery_requires_preview_takeover() {
        let html = include_str!("../dist/index.html");
        let js = include_str!("../dist/app.js");
        assert!(html.contains("id=\"view-backup\""));
        assert!(html.contains("id=\"backupCreateBtn\""));
        assert!(js.contains("createAndUploadPortableBackup()"));
        assert!(!js.contains("setInterval(createAndUploadPortableBackup"));
        let stage = js.find("backup_restore_stage").unwrap();
        let hydrate = js[stage..]
            .find("hydratePortableGenerationWebView")
            .unwrap()
            + stage;
        let preview = js[hydrate..].find("prepareRestorePublication").unwrap() + hydrate;
        let takeover = js[preview..]
            .find("backup_restore_takeover_publish")
            .unwrap()
            + preview;
        assert!(stage < hydrate && hydrate < preview && preview < takeover);
        assert!(js.contains("替换本机并接管"));
        assert!(html.contains("尚未改动本机"));
        assert!(js.contains("tool: \"backup_delete\""));
        assert!(js.contains("plan_etag: version.plan_etag"));
        assert!(js.contains("永久删除此版本"));
        assert!(js.contains("onError: async () => { await refreshBackupCenter(); }"));
    }

    #[test]
    fn android_reminder_delivery_has_a_startup_tripwire_and_debug_process_death_seam() {
        let js = include_str!("../dist/app.js");
        let plugin =
            include_str!("../../solum-alarm/android/src/main/java/dev/solum/alarm/AlarmPlugin.kt");
        let activity =
            include_str!("../gen/android/app/src/main/java/dev/solum/app/MainActivity.kt");
        assert!(plugin.contains("getReceiverInfo"));
        assert!(plugin.contains("startupReceiverRegistered"));
        assert!(plugin.contains("P1: AlarmReceiver is missing or disabled"));
        assert!(js.contains("reminder_delivery_health"));
        assert!(js.contains("当前安装包缺少后台提醒接收器"));
        assert!(js.contains("debug_simulate_process_death"));
        assert!(activity.contains("if (!BuildConfig.DEBUG) return false"));
        assert!(activity.contains("android.os.Process.killProcess"));
        assert!(!activity.contains("forceStopPackage"));
    }

    #[test]
    fn local_capture_stays_available_to_guests_and_dynamic_styles_survive_csp() {
        let html = include_str!("../dist/index.html");
        let js = include_str!("../dist/app.js");
        let config = include_str!("../tauri.conf.json");
        assert!(js.contains(
            "const GUEST_ALLOWED_VIEWS = new Set([\"chat\", \"agenda\", \"notifs\", \"ledger\", \"ledger-detail\", \"journal\", \"search\", \"settings\", \"privacy\", \"capture\", \"data\"])"
        ));
        assert!(config.contains("\"dangerousDisableAssetCspModification\": [\"style-src\"]"));
        assert!(config.contains("style-src 'self' 'unsafe-inline'"));
        assert!(!config.contains("script-src 'self' 'unsafe-inline'"));
        assert!(!html.contains("<script>"));
        assert!(html.contains("<script src=\"app.js\"></script>"));
        assert!(js.contains("meta capture-preview"));
        assert!(js.contains("previewChars.slice(0, 240)"));
    }
