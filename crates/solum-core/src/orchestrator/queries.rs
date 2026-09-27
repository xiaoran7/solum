use super::*;

impl Orchestrator {
    // ---- queries ----------------------------------------------------------

    pub fn agenda(&self, now: NaiveDateTime) -> Result<Vec<Event>> {
        self.store.upcoming_events(now)
    }

    pub fn all_events(&self) -> Result<Vec<Event>> {
        self.store.list_events()
    }

    pub fn event(&self, id: i64) -> Result<Event> {
        self.store.get_event(id)
    }

    pub fn due(&self, now: NaiveDateTime) -> Result<Vec<Notification>> {
        self.store.due_notifications(now)
    }

    /// Deliver everything that's due: mark each fired and return what fired.
    /// This is the reliability path — pure DB work, no LLM in the loop.
    pub fn fire_due(&mut self, now: NaiveDateTime) -> Result<Vec<Notification>> {
        let due = self.store.due_notifications(now)?;
        // Marking fired and journaling the fire are one unit: a journal
        // failure must not leave the reminder marked delivered with no trace
        // (or, after a crash between the two writes, unmarked but journaled).
        self.store.with_transaction(|s| {
            for n in &due {
                if let Some(id) = n.id {
                    s.mark_fired(id, now)?;
                    // F4: fired reminders also land in the behavior journal.
                    let title = s
                        .get_event(n.event_id)
                        .map(|ev| ev.title)
                        .unwrap_or_else(|_| format!("event#{}", n.event_id));
                    s.insert_behavior(&BehaviorEntry {
                        id: None,
                        ts: now,
                        kind: BehaviorKind::ReminderFired,
                        content: format!("提醒「{title}」已触发（提前{}）", n.lead_label),
                        source: Some(format!("notification#{id}")),
                    })?;
                }
            }
            Ok(())
        })?;
        Ok(due)
    }

    /// Snooze a reminder: ring again `minutes` from `now` (works on a pending
    /// one — postpone — and on one that already fired — "稍后再叫我"). Returns
    /// the new fire time. The Android alarm mirror converges on the next
    /// ticker pass, same as any other pending-set change.
    pub fn snooze(
        &mut self,
        notification_id: i64,
        minutes: i64,
        now: NaiveDateTime,
    ) -> Result<NaiveDateTime> {
        if !(1..=24 * 60).contains(&minutes) {
            return Err(CoreError::Invalid(format!(
                "snooze 时长必须在 1 分钟到 24 小时之间，得到 {minutes} 分钟"
            )));
        }
        let until = now + Duration::minutes(minutes);
        self.store.snooze_notification(notification_id, until)?;
        Ok(until)
    }

    /// Move an event to `new_start` (end, if any, shifts by the same delta),
    /// drop its still-pending reminders and re-plan them from the rule table.
    /// Fired/dismissed reminders stay as history. Returns the updated event
    /// and the fresh reminder plan.
    pub fn reschedule_event(
        &mut self,
        id: i64,
        new_start: NaiveDateTime,
        now: NaiveDateTime,
    ) -> Result<(Event, Vec<Notification>)> {
        let mut ev = self.store.get_event(id)?;
        let delta = new_start - ev.start;
        ev.start = new_start;
        ev.end = ev.end.map(|e| e + delta);
        let rule = self.rule_table.rule(ev.kind.clone());
        // Move the event, drop the old reminders and lay down the new ones as
        // one unit. Failing partway used to be able to leave the event moved
        // with its old reminders deleted and no new ones written — a rescheduled
        // event that never reminds anyone.
        let planned = plan_notifications(&ev, &rule, now, id);
        let stored = self.store.with_transaction(|s| {
            s.update_event_times(id, ev.start, ev.end)?;
            s.delete_pending_notifications_for_event(id)?;
            let mut stored = Vec::with_capacity(planned.len());
            for mut n in planned {
                let nid = s.insert_notification(&n)?;
                n.id = Some(nid);
                stored.push(n);
            }
            Ok(stored)
        })?;
        Ok((ev, stored))
    }

    /// Cancel (delete) an event and its reminders. Generic UI actions use the
    /// `event_cancel` Guard tool; this direct method remains for the separate
    /// notification-proposal flow, which binds approval to a version snapshot
    /// and TTL before calling it.
    pub fn cancel_event(&self, id: i64) -> Result<Event> {
        let ev = self.store.get_event(id)?;
        self.store.delete_event(id)?;
        self.audit_irreversible("event_cancel", &format!("删除日程「{}」及其提醒", ev.title));
        Ok(ev)
    }

    /// Record an irreversible local deletion in the append-only audit trail.
    ///
    /// The notification-proposal path has its own snapshot-bound confirmation
    /// rather than a Guard token, so its completed deletion still lands in the
    /// same append-only audit trail.
    ///
    /// Deliberately best-effort: failing to write an audit line must not turn
    /// a completed deletion into a reported error.
    pub(crate) fn audit_irreversible(&self, tool: &str, summary: &str) {
        let _ = self.store.append_audit(&crate::guard::AuditEntry {
            ts: self.store.wall_clock().unwrap_or_default(),
            tool: tool.to_string(),
            risk: RiskLevel::Dangerous,
            summary: summary.to_string(),
            decision: crate::guard::Decision::Executed,
            token_id: None,
            detail: "本机 UI 确认后执行（未经 Guard 令牌）".into(),
        });
    }

    /// Resolve "明天的会" against future events: an explicit date in the
    /// description narrows by day, the remaining words match against titles
    /// (either direction of containment — "会" finds "开会"). A key that
    /// matches nothing falls back to the date-scoped list rather than hiding
    /// everything behind a failed guess.
    pub(crate) fn find_event_targets(&self, desc: &str, now: NaiveDateTime) -> Result<Vec<Event>> {
        let mut cands = self.store.upcoming_events(now)?;
        let (date, _) = crate::time_parse::parse_date_time_parts(desc, now);
        if let Some(d) = date {
            cands.retain(|e| e.start.date() == d);
        }
        let mut key = desc.to_string();
        for f in TARGET_FILLERS {
            key = key.replace(f, "");
        }
        let key = key.trim();
        if !key.is_empty() {
            let hits: Vec<Event> = cands
                .iter()
                .filter(|e| e.title.contains(key) || key.contains(&e.title))
                .cloned()
                .collect();
            if !hits.is_empty() {
                return Ok(hits);
            }
        }
        Ok(cands)
    }

    /// Edit a memory fact's wording (F12: 可编辑). Recall reads the table
    /// directly, so the change is effective immediately.
    pub fn update_fact(&self, id: i64, content: &str) -> Result<()> {
        self.store.update_fact(id, content)
    }

    /// Export everything the user owns as one pretty-printed JSON document
    /// (§4: the data is theirs — backup / migration / off-device review).
    /// Read-only and fully offline.
    pub fn export_json(&self, now: NaiveDateTime) -> Result<String> {
        let doc = crate::export::build_export(&self.store, now)?;
        Ok(serde_json::to_string_pretty(&doc)?)
    }

    /// Build the fully validated v3 main payload from this Store and an
    /// immutable, same-profile platform snapshot. Read-only and fully offline;
    /// attachment bytes remain outside this JSON by protocol.
    pub fn export_portable_json(
        &self,
        now: NaiveDateTime,
        platform: crate::export_v3::PlatformPortableData,
    ) -> Result<String> {
        let doc = crate::export_v3::build_portable_export(&self.store, now, platform)?;
        Ok(serde_json::to_string_pretty(&doc)?)
    }
}
