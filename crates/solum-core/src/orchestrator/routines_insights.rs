use super::*;

impl Orchestrator {
    // ---- behavior journal & check-ins (F3/F4) ------------------------------

    /// The journal, newest first.
    pub fn behavior_log(&self) -> Result<Vec<BehaviorEntry>> {
        self.store.list_behavior()
    }

    /// If a proactive status check-in is due at `now` (per the
    /// `status_checkins` proactivity level), record the ask and return the
    /// question to surface. `None` means stay quiet. F13 (D5): a non-normal
    /// scene (sleeping / mid-event) also stays quiet — reminders still fire,
    /// only the *asking* is deferred.
    pub fn checkin_if_due(&mut self, now: NaiveDateTime) -> Result<Option<String>> {
        let level = self.proactivity.level(ProactivityDimension::StatusCheckins);
        let last = self.store.last_behavior_ts(BehaviorKind::CheckinAsked)?;
        if !checkin_due(level, last, now) {
            return Ok(None);
        }
        if self.current_scene(now)? != crate::scene::Scene::Normal {
            return Ok(None);
        }
        self.store.insert_behavior(&BehaviorEntry {
            id: None,
            ts: now,
            kind: BehaviorKind::CheckinAsked,
            content: CHECKIN_QUESTION.to_string(),
            source: None,
        })?;
        Ok(Some(CHECKIN_QUESTION.to_string()))
    }

    /// F13 v1: classify the current scene from schedule + sleep data.
    pub fn current_scene(&self, now: NaiveDateTime) -> Result<crate::scene::Scene> {
        let events = self.store.list_events()?;
        let sleep = self.store.list_health_samples_between(
            crate::wearable::HealthMetric::Sleep,
            now - Duration::hours(24),
            now,
        )?;
        Ok(crate::scene::scene(now, &events, &sleep))
    }

    // ---- suggestions (F10) --------------------------------------------------

    /// Run the rule engine over the schedule, the last 14 days of status
    /// entries, the wearable baselines (F11 wellness, D5), and the routine
    /// confirmation record (the D4 anti-nag brake), persisting anything new.
    /// Returns only newly-created suggestions (dedup keys keep reruns quiet).
    pub fn generate_suggestions(
        &mut self,
        now: NaiveDateTime,
        horizon_days: i64,
    ) -> Result<Vec<Suggestion>> {
        let events = self.store.list_events()?;
        let statuses = self.store.list_behavior_between(
            crate::journal::BehaviorKind::Status,
            now - Duration::days(14),
            now,
        )?;
        let mut all = suggest::generate(&events, &statuses, now, horizon_days);
        // Phase 8.1: Soulous rows are a distinct, read-only fact source.
        // They may provide F10 material, but are never copied into events,
        // memory_facts, recall, or the reminder delivery path.
        let soulous_facts = self.store.list_soulous_facts()?;
        all.extend(crate::soulous::generate_suggestions(
            &soulous_facts,
            now,
            horizon_days,
        ));
        // F11 v1: baseline-relative wellness signals. Each rule checks its own
        // metric's ≥14-day data gate, so an under-observed metric stays silent.
        let samples = self.store.list_health_samples()?;
        let baselines = crate::stats::baselines(&samples, now);
        all.extend(suggest::generate_wellness(&samples, &baselines, now));
        // D4 brake: routines unconfirmed for a week → offer to pause.
        let routines = self.store.list_routines()?;
        let recent_statuses = self.store.list_behavior_between(
            crate::journal::BehaviorKind::Status,
            now - Duration::days(7),
            now,
        )?;
        all.extend(suggest::generate_routine_pauses(
            &routines,
            &recent_statuses,
            now,
        ));

        let mut fresh = Vec::new();
        for mut s in all {
            if let Some(id) = self.store.insert_suggestion_if_new(&s)? {
                s.id = Some(id);
                fresh.push(s);
            }
        }
        Ok(fresh)
    }

    /// Auto-generation gated by the `life_suggestions` proactivity level:
    /// passive stays silent; secretary looks 1 day ahead; butler 3 days.
    /// F13: a non-normal scene (sleeping / mid-event) defers this round.
    pub fn auto_generate_suggestions(&mut self, now: NaiveDateTime) -> Result<Vec<Suggestion>> {
        let level = self
            .proactivity
            .level(ProactivityDimension::LifeSuggestions);
        match suggest::suggestion_horizon(level) {
            Some(days) => {
                if self.current_scene(now)? != crate::scene::Scene::Normal {
                    return Ok(Vec::new());
                }
                self.generate_suggestions(now, days)
            }
            None => Ok(Vec::new()),
        }
    }

    pub fn suggestions(&self) -> Result<Vec<Suggestion>> {
        self.store.list_suggestions()
    }

    /// Update a suggestion's status. Accepting closes loops for two kinds
    /// (D4): a habit suggestion auto-creates its routine (plus a semantic
    /// fact, §3.10 write source ②), and a pause suggestion deactivates its
    /// routine. Returns a follow-up message when a side effect happened.
    pub fn set_suggestion_status(
        &self,
        id: i64,
        status: SuggestionStatus,
        now: NaiveDateTime,
    ) -> Result<Option<String>> {
        let suggestion = self.store.get_suggestion(id)?;
        // Atomic pending→decided. A stale card must not be able to re-decide a
        // suggestion the user already answered, because accepting is not a
        // display change — it creates or pauses a routine.
        if !self.store.decide_suggestion(id, status)? {
            return Ok(Some("这条建议已经处理过了。".into()));
        }
        if status != SuggestionStatus::Accepted {
            return Ok(None);
        }
        match suggestion.kind {
            SuggestionKind::HabitReminder => {
                let Some((time, title)) = suggestion
                    .source
                    .as_deref()
                    .and_then(crate::routine::parse_habit_source)
                else {
                    return Ok(None);
                };
                let routine = Routine {
                    id: None,
                    title: title.clone(),
                    time_of_day: time.clone(),
                    source: Some(format!("suggestion#{id}")),
                    active: true,
                    created_at: now,
                    scheduled_until: None,
                };
                match self.store.insert_routine_if_new(&routine)? {
                    Some(rid) => {
                        // Solidify the habit as a semantic fact too (M2 ②).
                        let _ = self.store.insert_fact_if_new(&MemoryFact {
                            id: None,
                            content: format!("我通常在 {time} 左右{title}"),
                            source: "habit".into(),
                            created_at: now,
                            last_used_at: None,
                        })?;
                        Ok(Some(format!(
                            "已创建固定提醒 routine#{rid}：每天 {time}「{title}」。可在记忆台账停用或删除。"
                        )))
                    }
                    None => Ok(Some(format!("固定提醒「{title}」已存在，未重复创建。"))),
                }
            }
            SuggestionKind::RoutinePause => {
                let Some(rid) = suggestion
                    .source
                    .as_deref()
                    .and_then(|s| s.strip_prefix("routine#"))
                    .and_then(|s| s.parse::<i64>().ok())
                else {
                    return Ok(None);
                };
                self.store.set_routine_active(rid, false)?;
                Ok(Some(format!(
                    "已暂停 routine#{rid}（未删除，可在台账重新启用）。"
                )))
            }
            _ => Ok(None),
        }
    }

    // ---- routines (F3 完全体, D4) --------------------------------------------

    pub fn routines(&self) -> Result<Vec<Routine>> {
        self.store.list_routines()
    }

    pub fn set_routine_active(&mut self, id: i64, active: bool, now: NaiveDateTime) -> Result<()> {
        self.store.set_routine_active(id, active)?;
        if active {
            self.materialize_routines(now)?;
        }
        Ok(())
    }

    /// Change a routine's future-facing title/time. The store keeps fired
    /// occurrences as history and retracts only pending projections; rebuild
    /// immediately so the user never has to wait for the resident ticker.
    pub fn update_routine(
        &mut self,
        id: i64,
        title: &str,
        time_of_day: &str,
        now: NaiveDateTime,
    ) -> Result<()> {
        self.store.update_routine(id, title, time_of_day)?;
        self.materialize_routines(now)?;
        Ok(())
    }

    /// D4 的「一键已完成」：把一次 routine 完成确认落成行为日志的 Status 条目
    /// （`generate_routine_pauses` 认的就是这种确认）。按日去重——同一天重复
    /// 点击不再落新条目。
    pub fn confirm_routine(&mut self, id: i64, now: NaiveDateTime) -> Result<String> {
        let routine = self
            .store
            .list_routines()?
            .into_iter()
            .find(|r| r.id == Some(id))
            .ok_or_else(|| CoreError::Invalid(format!("routine#{id} 不存在")))?;
        let day_start = now.date().and_hms_opt(0, 0, 0).expect("valid midnight");
        // Match on provenance, not on the text: an ordinary status message that
        // merely repeats the routine's title ("我在护肤") is not a confirmation.
        let already = self
            .store
            .list_behavior_between(BehaviorKind::Status, day_start, now)?
            .iter()
            .any(|b| crate::routine::is_completion_of(b, id));
        if already {
            return Ok(format!("「{}」今天已经确认过了。", routine.title));
        }
        self.store.insert_behavior(&BehaviorEntry {
            id: None,
            ts: now,
            kind: BehaviorKind::Status,
            content: routine.title.clone(),
            source: Some(format!("routine#{id}")),
        })?;
        Ok(format!("已记录「{}」完成。", routine.title))
    }

    /// Materialize upcoming routine occurrences (today + tomorrow) as normal
    /// events + zero-lead notifications, so delivery reuses the whole existing
    /// pipeline (fire_due, AlarmManager mirror, sync, ledger). Idempotent:
    /// each routine tracks its scheduled high-water date, and occurrences are
    /// identified by routine provenance so nothing is ever duplicated.
    /// Returns how many occurrences were created or repaired.
    ///
    /// The high-water mark and repair have to coexist carefully. The mark is
    /// what stops a routine from resurrecting an occurrence **the user
    /// deleted** — so a date at or below it is never re-created from scratch.
    /// But it must not also suppress *repair*: an occurrence whose event is
    /// present while its notification is missing is not a user decision, it is
    /// a half-finished write, and leaving it alone means that day's reminder
    /// never fires and never can. So below the mark we repair only that exact
    /// shape (event present, no notification in any status), and a deleted
    /// event — no row at all — is respected as intentional.
    pub fn materialize_routines(&mut self, now: NaiveDateTime) -> Result<usize> {
        let mut created = 0;
        for r in self.store.list_routines()? {
            if !r.active {
                continue;
            }
            let Some(id) = r.id else { continue };
            let time = r.time()?;
            let mut high_water = r.scheduled_until;
            for day_offset in 0..2i64 {
                let date = now.date() + Duration::days(day_offset);
                let start = date.and_time(time);
                if high_water.is_some_and(|u| date <= u) {
                    // Already materialized through this date — repair only.
                    if start >= now
                        && self.store.routine_occurrence_event(id, start)?.is_some()
                        && self.store.routine_occurrence_needs_work(id, start)?
                    {
                        self.repair_routine_occurrence(id, start, now)?;
                        created += 1;
                    }
                    continue;
                }
                if start < now {
                    continue; // today's slot already passed — skip, don't backfire
                }
                // Ask by routine provenance, not by title+time. A *same-titled*
                // event that arrived via sync used to suppress the reminder for
                // a routine it has nothing to do with.
                if self.store.routine_occurrence_needs_work(id, start)? {
                    self.materialize_one(&r.title, id, start, now)?;
                    created += 1;
                }
                high_water = Some(date);
            }
            if high_water != r.scheduled_until {
                if let Some(until) = high_water {
                    self.store.set_routine_scheduled_until(id, until)?;
                }
            }
        }
        Ok(created)
    }

    /// Write one routine occurrence — the event (if absent) and its zero-lead
    /// notification — as a single transaction. Splitting these across two
    /// commits is what allowed "event written, reminder not" to become
    /// permanent.
    fn materialize_one(
        &self,
        title: &str,
        routine_id: i64,
        start: NaiveDateTime,
        now: NaiveDateTime,
    ) -> Result<()> {
        self.store.with_transaction(|s| {
            let ev_id = match s.routine_occurrence_event(routine_id, start)? {
                Some(existing) => existing,
                None => {
                    let ev = Event::new(
                        title.to_string(),
                        EventKind::Reminder,
                        start,
                        crate::routine::source_tag(routine_id),
                        now,
                    );
                    s.insert_event_with_scope(&ev, None, false, Some(routine_id))?
                }
            };
            s.insert_notification(&Notification {
                id: None,
                event_id: ev_id,
                fire_at: start,
                lead_label: "0m".into(),
                channels: vec![crate::model::Channel::Push],
                status: crate::model::NotificationStatus::Pending,
                created_at: now,
                fired_at: None,
            })?;
            Ok(())
        })
    }

    /// Re-attach the missing reminder to an occurrence whose event survived a
    /// half-finished write. Distinct from creating one: the event is known to
    /// exist, so this can never resurrect an occurrence the user deleted.
    fn repair_routine_occurrence(
        &self,
        routine_id: i64,
        start: NaiveDateTime,
        now: NaiveDateTime,
    ) -> Result<()> {
        let title = match self.store.routine_occurrence_event(routine_id, start)? {
            Some(ev_id) => self.store.get_event(ev_id)?.title,
            None => return Ok(()),
        };
        self.materialize_one(&title, routine_id, start, now)
    }

    // ---- semantic memory & recall (§3.10, M2/M3) ------------------------------

    /// All remembered facts, newest first.
    pub fn facts(&self) -> Result<Vec<MemoryFact>> {
        self.store.list_facts()
    }

    /// Local recall (M3): score facts, status-journal entries, and events
    /// against `query`, returning the capped top snippets. Notification-
    /// captured events are included only while their cloud setting is enabled
    /// and their capture-time scope is not `local_only` (§3.10 rule 2).
    pub fn recall(&self, query: &str, now: NaiveDateTime) -> Result<Vec<Snippet>> {
        let mut corpus = Vec::new();
        for f in self.store.list_facts()? {
            corpus.push(Candidate {
                layer: SnippetLayer::Fact,
                id: f.id.unwrap_or(0),
                content: f.content,
                created_at: f.created_at,
            });
        }
        for b in
            self.store
                .list_behavior_between(BehaviorKind::Status, now - Duration::days(60), now)?
        {
            corpus.push(Candidate {
                layer: SnippetLayer::Behavior,
                id: b.id.unwrap_or(0),
                content: b.content,
                created_at: b.ts,
            });
        }
        for e in self
            .store
            .list_recall_events(self.store.notif_cloud_enabled()?)?
        {
            let when = fmt_ts(&e.start);
            corpus.push(Candidate {
                layer: SnippetLayer::Event,
                id: e.id.unwrap_or(0),
                content: format!("{} {}", when, e.title),
                created_at: e.created_at,
            });
        }
        Ok(crate::recall::recall(query, &corpus, now))
    }

    /// The offline data review (D2): distributions + wearable baselines +
    /// the F11/F13 data gate. Read-only, never leaves the device.
    pub fn stats(&self, now: NaiveDateTime) -> Result<crate::stats::StatsReport> {
        let events = self.store.list_events()?;
        let behaviors = self.store.list_behavior()?;
        let samples = self.store.list_health_samples()?;
        Ok(crate::stats::build_stats(
            &events, &behaviors, &samples, now,
        ))
    }

    // ---- wearable health samples (F5, Phase 4) -------------------------------

    /// Hand the adapter's samples to storage, deduped. Pure storage — no
    /// cloud call, no proactivity coupling (that's F11, future work): a
    /// sample is inert data until something reads it. `created_at` is
    /// stamped with the injected clock, distinct from each sample's own
    /// `start`/`end` measurement time. Returns the count of newly-stored
    /// (non-duplicate) samples.
    pub fn record_health_samples(
        &mut self,
        samples: Vec<HealthSample>,
        now: NaiveDateTime,
    ) -> Result<usize> {
        let mut n = 0;
        for mut s in samples {
            s.created_at = now;
            if self.store.insert_health_sample_if_new(&s)?.is_some() {
                n += 1;
            }
        }
        Ok(n)
    }

    /// All stored samples, newest first (the F12 ledger's shape).
    pub fn health_samples(&self) -> Result<Vec<HealthSample>> {
        self.store.list_health_samples()
    }

    // ---- Soulous read-only facts (Phase 8.1) --------------------------------

    /// Pull every Soulous endpoint into one atomic local snapshot. Missing
    /// config is a quiet no-op; callers can expose that state in their own UI.
    pub fn pull_soulous(&self, now: NaiveDateTime) -> Result<Option<crate::soulous::PullOutcome>> {
        crate::soulous::pull_configured(&self.store, now)
    }

    pub fn soulous_facts(&self) -> Result<Vec<crate::soulous::SoulousFact>> {
        self.store.list_soulous_facts()
    }

    /// Feed actual imported exams through the normal importance-rule table.
    /// This is intentionally informative only: no remote record can create or
    /// alter Solum reminders without a user-authored Solum event.
    pub fn soulous_status(&self, now: NaiveDateTime) -> Result<crate::soulous::SoulousStatus> {
        let facts = self.store.list_soulous_facts()?;
        Ok(crate::soulous::build_status(&facts, &self.rule_table, now))
    }

    /// Cancel a pending reminder without deleting its event.
    pub fn dismiss(&self, notification_id: i64) -> Result<()> {
        self.store.dismiss_notification(notification_id)
    }

    pub fn ledger(&self) -> Result<Vec<MemoryEntry>> {
        self.store.memory_ledger()
    }

    /// Paged single-layer ledger slice for the F12 UI; see
    /// [`Store::memory_ledger_page`] for the cursor semantics.
    pub fn ledger_page(
        &self,
        layer: crate::model::MemoryLayer,
        limit: usize,
        cursor: Option<&str>,
    ) -> Result<(Vec<MemoryEntry>, Option<String>)> {
        self.store.memory_ledger_page(layer, limit, cursor)
    }

    pub fn forget(&self, layer: MemoryLayer, id: i64) -> Result<()> {
        self.store.delete_memory(layer, id)?;
        self.audit_irreversible("forget", &format!("删除记忆 {}#{id}", layer.as_str()));
        Ok(())
    }

    pub fn rule_table(&self) -> &RuleTable {
        &self.rule_table
    }

    /// Replace the short-term cloud-chat context when the shell switches
    /// between local conversation sessions. The transcript itself belongs to
    /// the shell's local-only session store; the core deliberately receives
    /// only the final few completed turns needed for the next reply.
    pub fn replace_chat_history(&mut self, turns: Vec<ChatTurn>) {
        let kept: Vec<ChatTurn> = turns
            .into_iter()
            .rev()
            .take(MAX_HISTORY_TURNS)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        self.chat_history = kept.into();
    }

    /// Save a user-edited importance rule and re-plan pending reminders for
    /// future events of that type. Fired/dismissed history remains untouched.
    /// The returned count is the number of events whose pending plan changed.
    pub fn set_importance_rule(
        &mut self,
        rule: crate::classify::ImportanceRule,
        now: NaiveDateTime,
    ) -> Result<usize> {
        let kind = rule.kind.clone();
        self.rule_table.set(rule);
        self.store.save_rule_table(&self.rule_table)?;

        let events = self.store.upcoming_events(now)?;
        // Per-event replanning is one unit each: deleting the old pending
        // reminders must never survive without the new plan when an insert
        // fails partway (that used to leave a silently unreminded event).
        let new_rule = self.rule_table.rule_ref(&kind);
        let mut replanned = 0;
        for event in events.into_iter().filter(|event| event.kind == kind) {
            let event_id = event.id.expect("stored events always carry an id");
            self.store.with_transaction(|s| {
                s.delete_pending_notifications_for_event(event_id)?;
                for notification in plan_notifications(&event, &new_rule, now, event_id) {
                    s.insert_notification(&notification)?;
                }
                Ok(())
            })?;
            replanned += 1;
        }
        Ok(replanned)
    }

    pub fn proactivity(&self) -> &ProactivityConfig {
        &self.proactivity
    }

    pub fn set_proactivity(
        &mut self,
        dim: ProactivityDimension,
        level: ProactivityLevel,
    ) -> Result<()> {
        self.proactivity.set(dim, level);
        self.store.save_proactivity(&self.proactivity)
    }

    /// Phase 9's device-local notification capture policy. The value affects
    /// subsequent captures; historical rows retain their original scope.
    pub fn notif_cloud_enabled(&self) -> Result<bool> {
        self.store.notif_cloud_enabled()
    }

    pub fn set_notif_cloud_enabled(&self, enabled: bool) -> Result<()> {
        self.store.set_notif_cloud_enabled(enabled)
    }
}
