use super::*;

impl Store {
    // ---- raw inputs -------------------------------------------------------

    pub fn insert_raw_input(
        &self,
        text: &str,
        intent: &str,
        created_at: NaiveDateTime,
    ) -> Result<i64> {
        self.insert_raw_input_with_scope(text, intent, created_at, false, "manual", None)
    }

    /// Store a raw input that must never enter the encrypted sync pipeline.
    /// Used only for text captured from third-party notifications.
    pub fn insert_local_only_raw_input(
        &self,
        text: &str,
        intent: &str,
        created_at: NaiveDateTime,
    ) -> Result<i64> {
        self.insert_raw_input_with_scope(text, intent, created_at, true, "notification", None)
    }

    /// Persist an input after the user has confirmed an external candidate.
    /// `source_id` is a stable registry id, not a display label.
    pub fn insert_raw_input_from_source(
        &self,
        text: &str,
        intent: &str,
        created_at: NaiveDateTime,
        source_id: &str,
        external_id: Option<&str>,
        local_only: bool,
    ) -> Result<i64> {
        self.insert_raw_input_with_scope(
            text,
            intent,
            created_at,
            local_only,
            source_id,
            external_id,
        )
    }

    pub(crate) fn insert_raw_input_with_scope(
        &self,
        text: &str,
        intent: &str,
        created_at: NaiveDateTime,
        local_only: bool,
        source_id: &str,
        external_id: Option<&str>,
    ) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO raw_inputs(text, intent, created_at, local_only, source_id, external_id, guid)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                text,
                intent,
                fmt_ts(&created_at),
                local_only as i64,
                source_id,
                external_id,
                new_guid()
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// All raw inputs, oldest first (for the data export).
    pub fn list_raw_inputs(&self) -> Result<Vec<RawInputRow>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, text, intent, created_at, source_id, external_id FROM raw_inputs ORDER BY id ASC")?;
        let rows = stmt.query_map([], |row| {
            Ok(RawInputRow {
                id: row.get(0)?,
                text: row.get(1)?,
                intent: row.get(2)?,
                created_at: row.get(3)?,
                source_id: row.get(4)?,
                external_id: row.get(5)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Content-free activity projection for the information-source center.
    /// The UI needs a count and last success time, never the source text.
    pub fn raw_input_source_activity(&self, source_id: &str) -> Result<(i64, Option<String>)> {
        self.conn
            .query_row(
                "SELECT COUNT(*), MAX(created_at) FROM raw_inputs WHERE source_id = ?1",
                params![source_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(Into::into)
    }

    pub fn accepted_source_item(
        &self,
        source_id: &str,
        external_id: &str,
    ) -> Result<Option<(i64, Option<Event>)>> {
        let raw_id = self
            .conn
            .query_row(
                "SELECT id FROM raw_inputs WHERE source_id = ?1 AND external_id = ?2 ORDER BY id LIMIT 1",
                params![source_id, external_id],
                |row| row.get(0),
            )
            .optional()?;
        let Some(raw_id) = raw_id else {
            return Ok(None);
        };
        let event = self
            .conn
            .query_row(
                "SELECT * FROM events WHERE raw_input_id = ?1 ORDER BY id LIMIT 1",
                params![raw_id],
                Self::row_to_event,
            )
            .optional()?;
        Ok(Some((raw_id, event)))
    }

    /// Stable raw-input identity for the v3 logical backup journal. This is a
    /// read-only relation translation; SQLite row ids never enter the payload.
    pub(crate) fn raw_input_guid_for_backup(&self, id: i64) -> Result<String> {
        self.conn
            .query_row(
                "SELECT guid FROM raw_inputs WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .optional()?
            .ok_or_else(|| CoreError::NotFound(format!("raw_input#{id}")))
    }

    /// Optional event identity for notification history. Historical captures
    /// remain exportable after their event has been deleted.
    pub(crate) fn event_guid_for_backup(&self, id: i64) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT guid FROM events WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .optional()?)
    }

    // ---- events -----------------------------------------------------------

    pub fn insert_event(&self, ev: &Event, raw_input_id: Option<i64>) -> Result<i64> {
        self.insert_event_with_scope(ev, raw_input_id, false, None)
    }

    /// Insert an event with its sync scope and optional routine provenance.
    /// Local-only events are derived from third-party notification text and are
    /// deliberately excluded from sync triggers.
    pub fn insert_event_with_scope(
        &self,
        ev: &Event,
        raw_input_id: Option<i64>,
        local_only: bool,
        routine_id: Option<i64>,
    ) -> Result<i64> {
        let people = serde_json::to_string(&ev.people)?;
        self.conn.execute(
            "INSERT INTO events(title, kind, start, end, location, people_json, raw_input_id, routine_id, created_at, local_only, guid)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                ev.title,
                ev.kind.as_str(),
                fmt_ts(&ev.start),
                ev.end.map(|d| fmt_ts(&d)),
                ev.location,
                people,
                raw_input_id,
                routine_id,
                fmt_ts(&ev.created_at),
                local_only as i64,
                new_guid(),
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub(crate) fn row_to_event(row: &rusqlite::Row) -> rusqlite::Result<Event> {
        let kind_s: String = row.get("kind")?;
        let start_s: String = row.get("start")?;
        let end_s: Option<String> = row.get("end")?;
        let people_s: String = row.get("people_json")?;
        let created_s: String = row.get("created_at")?;
        let id_for_log: i64 = row.get("id")?;
        let map = |e: CoreError| rusqlite::Error::ToSqlConversionFailure(Box::new(e));
        // Corrupt people_json must not silently read as "no participants":
        // surface it (id + raw bytes) and keep the empty fallback so one bad
        // row cannot take down list_events for the whole ledger.
        let people: Vec<String> = match serde_json::from_str(&people_s) {
            Ok(people) => people,
            Err(error) => {
                eprintln!(
                    "[store] events#{}: corrupt people_json ({error}); treating as empty",
                    id_for_log
                );
                Vec::new()
            }
        };
        Ok(Event {
            id: Some(id_for_log),
            title: row.get("title")?,
            kind: kind_s.parse().map_err(map)?,
            start: parse_ts(&start_s).map_err(map)?,
            end: match end_s {
                Some(s) => Some(parse_ts(&s).map_err(map)?),
                None => None,
            },
            location: row.get("location")?,
            people,
            raw_input: String::new(),
            created_at: parse_ts(&created_s).map_err(map)?,
        })
    }

    pub fn get_event(&self, id: i64) -> Result<Event> {
        self.conn
            .query_row("SELECT * FROM events WHERE id = ?1", params![id], |r| {
                Self::row_to_event(r)
            })
            .optional()?
            .ok_or_else(|| CoreError::NotFound(format!("event#{id}")))
    }

    /// Stable identity and privacy scope for an outbound event projection.
    /// `Event` intentionally stays display-domain-only; callers must obtain
    /// these two storage concerns explicitly before any data can leave Solum.
    pub fn event_guid_and_local_only(&self, id: i64) -> Result<(String, bool)> {
        self.conn
            .query_row(
                "SELECT guid, local_only FROM events WHERE id = ?1",
                params![id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? != 0)),
            )
            .optional()?
            .ok_or_else(|| CoreError::NotFound(format!("event#{id}")))
    }

    pub fn list_events(&self) -> Result<Vec<Event>> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM events ORDER BY start ASC")?;
        let rows = stmt.query_map([], Self::row_to_event)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Events starting at or after `now`, soonest first.
    pub fn upcoming_events(&self, now: NaiveDateTime) -> Result<Vec<Event>> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM events WHERE start >= ?1 ORDER BY start ASC")?;
        let rows = stmt.query_map(params![fmt_ts(&now)], Self::row_to_event)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Move an event's start (and end, pre-shifted by the caller). Sync
    /// captures the UPDATE via the existing triggers.
    pub fn update_event_times(
        &self,
        id: i64,
        start: NaiveDateTime,
        end: Option<NaiveDateTime>,
    ) -> Result<()> {
        let n = self.conn.execute(
            "UPDATE events SET start = ?1, end = ?2 WHERE id = ?3",
            params![fmt_ts(&start), end.map(|d| fmt_ts(&d)), id],
        )?;
        if n == 0 {
            return Err(CoreError::NotFound(format!("event#{id}")));
        }
        Ok(())
    }

    /// Drop the still-pending reminders of an event (reschedule re-plans them
    /// from the rule table; fired/dismissed rows stay as history).
    pub fn delete_pending_notifications_for_event(&self, event_id: i64) -> Result<usize> {
        let n = self.conn.execute(
            "DELETE FROM notifications WHERE event_id = ?1 AND status = 'pending'",
            params![event_id],
        )?;
        Ok(n)
    }

    /// Delete an event and its notifications.
    /// Delete an event and its reminders together. Atomic: failing between the
    /// two steps would leave the event with its reminders already gone —
    /// visibly scheduled, silently never firing.
    pub fn delete_event(&self, id: i64) -> Result<()> {
        self.with_transaction(|s| {
            s.conn
                .execute("DELETE FROM notifications WHERE event_id = ?1", params![id])?;
            let n = s
                .conn
                .execute("DELETE FROM events WHERE id = ?1", params![id])?;
            if n == 0 {
                return Err(CoreError::NotFound(format!("event#{id}")));
            }
            Ok(())
        })
    }

    // ---- notifications ----------------------------------------------------

    pub fn insert_notification(&self, n: &Notification) -> Result<i64> {
        let channels = serde_json::to_string(&n.channels)?;
        self.conn.execute(
            "INSERT INTO notifications(event_id, fire_at, lead_label, channels_json, status, created_at, fired_at, local_only, guid)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7,
                     COALESCE((SELECT local_only FROM events WHERE id = ?1), 0), ?8)",
            params![
                n.event_id,
                fmt_ts(&n.fire_at),
                n.lead_label,
                channels,
                n.status.as_str(),
                fmt_ts(&n.created_at),
                n.fired_at.map(|d| fmt_ts(&d)),
                new_guid(),
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub(crate) fn row_to_notification(row: &rusqlite::Row) -> rusqlite::Result<Notification> {
        let fire_s: String = row.get("fire_at")?;
        let channels_s: String = row.get("channels_json")?;
        let status_s: String = row.get("status")?;
        let created_s: String = row.get("created_at")?;
        let fired_s: Option<String> = row.get("fired_at")?;
        let id_for_log: i64 = row.get("id")?;
        let map = |e: CoreError| rusqlite::Error::ToSqlConversionFailure(Box::new(e));
        // Same policy as people_json above: a corrupt channels_json is logged
        // with the row id instead of silently reading as "no channels".
        let channels: Vec<Channel> = match serde_json::from_str(&channels_s) {
            Ok(channels) => channels,
            Err(error) => {
                eprintln!(
                    "[store] notifications#{id_for_log}: corrupt channels_json ({error}); treating as empty"
                );
                Vec::new()
            }
        };
        Ok(Notification {
            id: Some(id_for_log),
            event_id: row.get("event_id")?,
            fire_at: parse_ts(&fire_s).map_err(map)?,
            lead_label: row.get("lead_label")?,
            channels,
            status: status_s.parse().map_err(map)?,
            created_at: parse_ts(&created_s).map_err(map)?,
            fired_at: match fired_s {
                Some(s) => Some(parse_ts(&s).map_err(map)?),
                None => None,
            },
        })
    }

    pub fn list_notifications(&self) -> Result<Vec<Notification>> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM notifications ORDER BY fire_at ASC")?;
        let rows = stmt.query_map([], Self::row_to_notification)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Pending notifications whose fire time has arrived, earliest first.
    pub fn due_notifications(&self, now: NaiveDateTime) -> Result<Vec<Notification>> {
        let mut stmt = self.conn.prepare(
            "SELECT * FROM notifications WHERE status = 'pending' AND fire_at <= ?1 ORDER BY fire_at ASC",
        )?;
        let rows = stmt.query_map(params![fmt_ts(&now)], Self::row_to_notification)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Cancel a still-pending notification so it never fires (without deleting
    /// the event it belongs to).
    pub fn dismiss_notification(&self, id: i64) -> Result<()> {
        let n = self.conn.execute(
            "UPDATE notifications SET status = ?1 WHERE id = ?2 AND status = 'pending'",
            params![NotificationStatus::Dismissed.as_str(), id],
        )?;
        if n == 0 {
            return Err(CoreError::NotFound(format!(
                "pending notification#{id} (already fired/dismissed, or missing)"
            )));
        }
        Ok(())
    }

    /// Push a reminder's fire time to `until` and re-arm it (snooze). Works on
    /// a pending reminder (postpone) and on a fired one (ring again later);
    /// a dismissed reminder stays cancelled — un-cancelling is a separate,
    /// deliberate act, not something a snooze should do by accident.
    pub fn snooze_notification(&self, id: i64, until: NaiveDateTime) -> Result<()> {
        let n = self.conn.execute(
            "UPDATE notifications SET fire_at = ?1, status = 'pending', fired_at = NULL
             WHERE id = ?2 AND status IN ('pending', 'fired')",
            params![fmt_ts(&until), id],
        )?;
        if n == 0 {
            return Err(CoreError::NotFound(format!(
                "notification#{id}（不存在或已被取消）"
            )));
        }
        Ok(())
    }

    pub fn mark_fired(&self, id: i64, now: NaiveDateTime) -> Result<()> {
        let n = self.conn.execute(
            "UPDATE notifications SET status = ?1, fired_at = ?2 WHERE id = ?3",
            params![NotificationStatus::Fired.as_str(), fmt_ts(&now), id],
        )?;
        if n == 0 {
            return Err(CoreError::NotFound(format!("notification#{id}")));
        }
        Ok(())
    }

    /// Count raw inputs recorded within `[from, to]` (for the review digest).
    pub fn count_raw_inputs_between(
        &self,
        from: NaiveDateTime,
        to: NaiveDateTime,
    ) -> Result<usize> {
        let n: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM raw_inputs WHERE created_at >= ?1 AND created_at <= ?2",
            params![fmt_ts(&from), fmt_ts(&to)],
            |r| r.get(0),
        )?;
        Ok(n as usize)
    }
}
