use super::*;

impl Store {
    // ---- memory ledger (F12) ---------------------------------------------

    /// A unified, newest-first view of everything the agent remembers, across
    /// layers. This is the surface the user inspects and prunes.
    ///
    /// Whole-ledger callers (export, tests) only. The F12 UI must use
    /// [`Store::memory_ledger_page`] — this loads every table in full and
    /// sorts in memory, which is unbounded on a long-lived device.
    pub fn memory_ledger(&self) -> Result<Vec<MemoryEntry>> {
        let mut entries = Vec::new();

        let mut stmt = self
            .conn
            .prepare("SELECT id, text, intent, created_at, source_id FROM raw_inputs")?;
        let rows = stmt.query_map([], |r| {
            let id: i64 = r.get(0)?;
            let text: String = r.get(1)?;
            let intent: String = r.get(2)?;
            let created: String = r.get(3)?;
            let source_id: String = r.get(4)?;
            Ok((id, text, intent, created, source_id))
        })?;
        for row in rows {
            let (id, text, intent, created, source_id) = row?;
            entries.push(MemoryEntry {
                layer: MemoryLayer::RawInput,
                id,
                summary: text,
                source: Some(format!("source={source_id} · intent={intent}")),
                created_at: parse_ts(&created)?,
            });
        }

        entries.extend(self.ledger_events(None, usize::MAX)?);
        entries.extend(self.ledger_notifications(None, usize::MAX)?);
        entries.extend(self.ledger_behavior(None, usize::MAX)?);
        entries.extend(self.ledger_suggestions(None, usize::MAX)?);
        entries.extend(self.ledger_wearable(None, usize::MAX)?);
        entries.extend(self.ledger_facts(None, usize::MAX)?);
        entries.extend(self.ledger_routines(None, usize::MAX)?);
        entries.extend(self.ledger_captures(None, usize::MAX)?);

        entries.sort_by(|a, b| b.created_at.cmp(&a.created_at).then(a.id.cmp(&b.id)));
        Ok(entries)
    }

    /// One F12 ledger page: newest-first entries of a single layer, keyset
    /// paginated so the UI never loads a long-lived device's whole table.
    ///
    /// `cursor` is the `next_cursor` from the previous call (None for the
    /// first page). The returned cursor is None once this layer is exhausted.
    /// Rows written while paging are not revisited; rows deleted between
    /// pages just shrink later pages.
    pub fn memory_ledger_page(
        &self,
        layer: MemoryLayer,
        limit: usize,
        cursor: Option<&str>,
    ) -> Result<(Vec<MemoryEntry>, Option<String>)> {
        if limit == 0 || limit > 500 {
            return Err(CoreError::Invalid("台账分页每页须为 1..=500 条".into()));
        }
        // Fetch one extra row to detect "there is more" without a count query.
        let probe = limit + 1;
        let after = cursor.map(LedgerCursor::decode).transpose()?;

        let mut entries = match layer {
            MemoryLayer::RawInput => {
                // raw_inputs has no list helper that preserves intent/source
                // metadata, so its row builder takes the statement directly.
                let mut stmt = self.conn.prepare(&format!(
                    "SELECT * FROM raw_inputs {}",
                    Self::LEDGER_KEYSET_TAIL.replace("{col}", "created_at")
                ))?;
                let rows = stmt.query_map(Self::keyset_params(after.as_ref(), probe), |r| {
                    Ok((
                        r.get::<_, i64>("id")?,
                        r.get::<_, String>("text")?,
                        r.get::<_, String>("intent")?,
                        r.get::<_, String>("created_at")?,
                        r.get::<_, String>("source_id")?,
                    ))
                })?;
                let mut entries = Vec::new();
                for row in rows {
                    let (id, text, intent, created, source_id) = row?;
                    entries.push(MemoryEntry {
                        layer,
                        id,
                        summary: text,
                        source: Some(format!("source={source_id} · intent={intent}")),
                        created_at: parse_ts(&created)?,
                    });
                }
                entries
            }
            MemoryLayer::Event => self.ledger_events(after.as_ref(), probe)?,
            MemoryLayer::Notification => self.ledger_notifications(after.as_ref(), probe)?,
            MemoryLayer::Behavior => self.ledger_behavior(after.as_ref(), probe)?,
            MemoryLayer::Suggestion => self.ledger_suggestions(after.as_ref(), probe)?,
            MemoryLayer::Wearable => self.ledger_wearable(after.as_ref(), probe)?,
            MemoryLayer::Fact => self.ledger_facts(after.as_ref(), probe)?,
            MemoryLayer::Routine => self.ledger_routines(after.as_ref(), probe)?,
            MemoryLayer::NotificationCapture => self.ledger_captures(after.as_ref(), probe)?,
        };

        let next_cursor = if entries.len() > limit {
            entries.truncate(limit);
            let last = &entries[limit - 1];
            Some(
                LedgerCursor {
                    created_at_str: fmt_ts(&last.created_at),
                    excl_id: last.id,
                }
                .encode(),
            )
        } else {
            None
        };
        Ok((entries, next_cursor))
    }

    /// Shared WHERE/ORDER/LIMIT tail for the per-layer paged queries. The
    /// three-part keyset predicate keeps exactly the ordering the full-ledger
    /// sort uses; NULL params mean "first page". `{col}` is substituted by
    /// each helper (notification_captures pages on received_at, behavior on
    /// ts).
    const LEDGER_KEYSET_TAIL: &str = "WHERE (?1 IS NULL OR {col} < ?2 OR ({col} = ?2 AND id > ?3))
         ORDER BY {col} DESC, id ASC LIMIT ?4";

    pub(crate) fn keyset_params(
        after: Option<&LedgerCursor>,
        probe: usize,
    ) -> [rusqlite::types::Value; 4] {
        [
            after.map(|c| c.created_at_str.clone()).into(),
            after.map(|c| c.created_at_str.clone()).into(),
            after.map(|c| c.excl_id).into(),
            (probe as i64).into(),
        ]
    }

    // Per-layer paged reads behind memory_ledger_page. Each maps rows through
    // exactly the summary formatting the full ledger uses, so a paged UI and
    // an export of the same row render identically.

    pub(crate) fn ledger_events(
        &self,
        after: Option<&LedgerCursor>,
        probe: usize,
    ) -> Result<Vec<MemoryEntry>> {
        let tail = Self::LEDGER_KEYSET_TAIL.replace("{col}", "created_at");
        let mut stmt = self.conn.prepare(&format!("SELECT * FROM events {tail}"))?;
        let rows = stmt.query_map(Self::keyset_params(after, probe), Self::row_to_event)?;
        let events = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(events
            .into_iter()
            .map(|ev| {
                let loc = ev
                    .location
                    .as_ref()
                    .map(|l| format!(" @{l}"))
                    .unwrap_or_default();
                MemoryEntry {
                    layer: MemoryLayer::Event,
                    id: ev.id.unwrap_or(0),
                    summary: format!(
                        "[{}] {} — {}{}",
                        ev.kind.label(),
                        ev.title,
                        crate::model::fmt_ts_human(&ev.start),
                        loc
                    ),
                    source: None,
                    created_at: ev.created_at,
                }
            })
            .collect())
    }

    pub(crate) fn ledger_notifications(
        &self,
        after: Option<&LedgerCursor>,
        probe: usize,
    ) -> Result<Vec<MemoryEntry>> {
        let tail = Self::LEDGER_KEYSET_TAIL.replace("{col}", "created_at");
        let mut stmt = self
            .conn
            .prepare(&format!("SELECT * FROM notifications {tail}"))?;
        let rows = stmt.query_map(Self::keyset_params(after, probe), Self::row_to_notification)?;
        let notifications = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(notifications
            .into_iter()
            .map(|n| MemoryEntry {
                layer: MemoryLayer::Notification,
                id: n.id.unwrap_or(0),
                summary: format!(
                    "提前 {} 通知 event#{} @ {}（{}）",
                    n.lead_label,
                    n.event_id,
                    crate::model::fmt_ts_human(&n.fire_at),
                    match n.status {
                        NotificationStatus::Pending => "待触发",
                        NotificationStatus::Fired => "已触发",
                        NotificationStatus::Dismissed => "已取消",
                    }
                ),
                source: Some(format!("event#{}", n.event_id)),
                created_at: n.created_at,
            })
            .collect())
    }

    pub(crate) fn ledger_behavior(
        &self,
        after: Option<&LedgerCursor>,
        probe: usize,
    ) -> Result<Vec<MemoryEntry>> {
        let tail = Self::LEDGER_KEYSET_TAIL.replace("{col}", "ts");
        let mut stmt = self
            .conn
            .prepare(&format!("SELECT * FROM behavior_log {tail}"))?;
        let rows = stmt.query_map(Self::keyset_params(after, probe), |r| {
            Ok((
                r.get::<_, i64>("id")?,
                r.get::<_, String>("ts")?,
                r.get::<_, String>("kind")?,
                r.get::<_, String>("content")?,
                r.get::<_, Option<String>>("source")?,
            ))
        })?;
        let entries = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        entries
            .into_iter()
            .map(|(id, ts, kind, content, source)| {
                let label = match kind.as_str() {
                    "status" => "状态",
                    "checkin_asked" => "问询",
                    "reminder_fired" => "提醒触发",
                    _ => "行为",
                };
                Ok(MemoryEntry {
                    layer: MemoryLayer::Behavior,
                    id,
                    summary: format!("[{label}] {content}"),
                    source,
                    created_at: parse_ts(&ts)?,
                })
            })
            .collect()
    }

    pub(crate) fn ledger_suggestions(
        &self,
        after: Option<&LedgerCursor>,
        probe: usize,
    ) -> Result<Vec<MemoryEntry>> {
        let tail = Self::LEDGER_KEYSET_TAIL.replace("{col}", "created_at");
        let mut stmt = self
            .conn
            .prepare(&format!("SELECT * FROM suggestions {tail}"))?;
        let rows = stmt.query_map(Self::keyset_params(after, probe), Self::row_to_suggestion)?;
        let suggestions = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(suggestions
            .into_iter()
            .map(|s| MemoryEntry {
                layer: MemoryLayer::Suggestion,
                id: s.id.unwrap_or(0),
                summary: format!(
                    "[{}] {}",
                    match s.status {
                        crate::suggest::SuggestionStatus::Pending => "待处理",
                        crate::suggest::SuggestionStatus::Accepted => "已采纳",
                        crate::suggest::SuggestionStatus::Dismissed => "已忽略",
                    },
                    s.text
                ),
                source: s.source,
                created_at: s.created_at,
            })
            .collect())
    }

    pub(crate) fn ledger_wearable(
        &self,
        after: Option<&LedgerCursor>,
        probe: usize,
    ) -> Result<Vec<MemoryEntry>> {
        let tail = Self::LEDGER_KEYSET_TAIL.replace("{col}", "created_at");
        let mut stmt = self
            .conn
            .prepare(&format!("SELECT * FROM health_samples {tail}"))?;
        let rows = stmt.query_map(
            Self::keyset_params(after, probe),
            Self::row_to_health_sample,
        )?;
        let samples = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(samples
            .into_iter()
            .map(|h| {
                let unit = match h.kind {
                    HealthMetric::HeartRate => "bpm",
                    HealthMetric::Steps => "步",
                    HealthMetric::Sleep => "分钟",
                };
                MemoryEntry {
                    layer: MemoryLayer::Wearable,
                    id: h.id.unwrap_or(0),
                    summary: format!(
                        "[{}] {} {unit} @ {}",
                        h.kind.label(),
                        h.value,
                        crate::model::fmt_ts_human(&h.start)
                    ),
                    source: Some(h.source),
                    created_at: h.created_at,
                }
            })
            .collect())
    }

    pub(crate) fn ledger_facts(
        &self,
        after: Option<&LedgerCursor>,
        probe: usize,
    ) -> Result<Vec<MemoryEntry>> {
        let tail = Self::LEDGER_KEYSET_TAIL.replace("{col}", "created_at");
        let mut stmt = self
            .conn
            .prepare(&format!("SELECT * FROM memory_facts {tail}"))?;
        let rows = stmt.query_map(Self::keyset_params(after, probe), Self::row_to_fact)?;
        let facts = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(facts
            .into_iter()
            .map(|f| MemoryEntry {
                layer: MemoryLayer::Fact,
                id: f.id.unwrap_or(0),
                summary: f.content,
                source: Some(f.source),
                created_at: f.created_at,
            })
            .collect())
    }

    pub(crate) fn ledger_routines(
        &self,
        after: Option<&LedgerCursor>,
        probe: usize,
    ) -> Result<Vec<MemoryEntry>> {
        let tail = Self::LEDGER_KEYSET_TAIL.replace("{col}", "created_at");
        let mut stmt = self
            .conn
            .prepare(&format!("SELECT * FROM routines {tail}"))?;
        let rows = stmt.query_map(Self::keyset_params(after, probe), Self::row_to_routine)?;
        let routines = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(routines
            .into_iter()
            .map(|r| MemoryEntry {
                layer: MemoryLayer::Routine,
                id: r.id.unwrap_or(0),
                summary: format!(
                    "[{}] 每天 {} 「{}」",
                    if r.active { "启用" } else { "已暂停" },
                    r.time_of_day,
                    r.title
                ),
                source: r.source,
                created_at: r.created_at,
            })
            .collect())
    }

    pub(crate) fn ledger_captures(
        &self,
        after: Option<&LedgerCursor>,
        probe: usize,
    ) -> Result<Vec<MemoryEntry>> {
        // notification_captures orders by received_at rather than created_at.
        let mut stmt = self.conn.prepare(
            "SELECT * FROM notification_captures
             WHERE (?1 IS NULL OR received_at < ?2 OR (received_at = ?2 AND id > ?3))
             ORDER BY received_at DESC, id ASC LIMIT ?4",
        )?;
        let rows = stmt.query_map(
            Self::keyset_params(after, probe),
            Self::row_to_notification_capture,
        )?;
        let captures = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(captures
            .into_iter()
            .map(|capture| {
                let lane = match capture.lane {
                    CaptureLane::Urgent => "重要·即时",
                    CaptureLane::Batch => "普通·批量",
                };
                let state = match capture.state {
                    CaptureState::Queued => "待处理",
                    CaptureState::EventCreated => "已建事件",
                    CaptureState::Filtered => "已过滤",
                    CaptureState::Deduplicated => "已判重",
                    CaptureState::NeedsReview => "待回看",
                    CaptureState::Resolved => "已处理",
                };
                let reason = capture
                    .reason
                    .as_deref()
                    .map(|reason| format!(" · {reason}"))
                    .unwrap_or_default();
                MemoryEntry {
                    layer: MemoryLayer::NotificationCapture,
                    id: capture.id.unwrap_or(0),
                    summary: format!("[{lane}·{state}] {}", capture.title),
                    source: Some(format!("{}{}", capture.package_name, reason)),
                    created_at: capture.received_at,
                }
            })
            .collect())
    }

    ///
    /// The Guard's preview is only worth showing if it names the *whole*
    /// blast radius, and for `raw_input` that is not one row: the cascade
    /// removes every event derived from it and every reminder under those.
    /// A preview that said "删除 1 条原始输入" while five reminders quietly
    /// went with it would be exactly the kind of under-stated confirmation
    /// the effect-digest binding exists to prevent.
    pub fn describe_memory_deletion(&self, layer: MemoryLayer, id: i64) -> Result<String> {
        if layer != MemoryLayer::RawInput {
            return Ok(String::new());
        }
        let events: i64 = self.conn.query_row(
            "SELECT count(*) FROM events WHERE raw_input_id = ?1",
            params![id],
            |r| r.get(0),
        )?;
        let notifs: i64 = self.conn.query_row(
            "SELECT count(*) FROM notifications WHERE event_id IN
             (SELECT id FROM events WHERE raw_input_id = ?1)",
            params![id],
            |r| r.get(0),
        )?;
        let captures: i64 = self.conn.query_row(
            "SELECT count(*) FROM notification_captures WHERE raw_input_id = ?1",
            params![id],
            |r| r.get(0),
        )?;
        if events == 0 && notifs == 0 && captures == 0 {
            return Ok(String::new());
        }
        let mut parts = Vec::new();
        if events > 0 {
            parts.push(format!("{events} 条日程"));
        }
        if notifs > 0 {
            parts.push(format!("{notifs} 条提醒"));
        }
        if captures > 0 {
            parts.push(format!("{captures} 条通知捕获"));
        }
        Ok(format!("，连同由它派生的 {}", parts.join("、")))
    }

    /// One-line summary of a ledger row, for confirmation previews.
    pub fn memory_summary(&self, layer: MemoryLayer, id: i64) -> Result<Option<String>> {
        let (table, col) = match layer {
            MemoryLayer::RawInput => ("raw_inputs", "text"),
            MemoryLayer::Event => ("events", "title"),
            MemoryLayer::Behavior => ("behavior_log", "content"),
            MemoryLayer::Suggestion => ("suggestions", "text"),
            MemoryLayer::Fact => ("memory_facts", "content"),
            MemoryLayer::Routine => ("routines", "title"),
            MemoryLayer::NotificationCapture => ("notification_captures", "title"),
            // No single human-readable column; the layer label carries enough.
            MemoryLayer::Notification | MemoryLayer::Wearable => return Ok(None),
        };
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {col} FROM {table} WHERE id = ?1"),
                params![id],
                |r| r.get::<_, String>(0),
            )
            .optional()?)
    }

    pub fn delete_memory(&self, layer: MemoryLayer, id: i64) -> Result<()> {
        match layer {
            MemoryLayer::Behavior => self.delete_behavior(id)?,
            MemoryLayer::Suggestion => self.delete_suggestion(id)?,
            MemoryLayer::Wearable => self.delete_health_sample(id)?,
            MemoryLayer::Fact => self.delete_fact(id)?,
            MemoryLayer::Routine => self.delete_routine(id)?,
            MemoryLayer::NotificationCapture => self.delete_notification_capture(id)?,
            MemoryLayer::Notification => {
                let n = self
                    .conn
                    .execute("DELETE FROM notifications WHERE id = ?1", params![id])?;
                if n == 0 {
                    return Err(CoreError::NotFound(format!("notification#{id}")));
                }
            }
            MemoryLayer::Event => self.delete_event(id)?,
            MemoryLayer::RawInput => self.delete_raw_input_cascade(id)?,
        }
        Ok(())
    }
}
