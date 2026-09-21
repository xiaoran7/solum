use super::*;

impl Store {
    // ---- behavior journal (F4) ---------------------------------------------

    pub fn insert_behavior(&self, e: &BehaviorEntry) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO behavior_log(ts, kind, content, source, guid) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                fmt_ts(&e.ts),
                e.kind.as_str(),
                e.content,
                e.source,
                new_guid()
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub(crate) fn row_to_behavior(row: &rusqlite::Row) -> rusqlite::Result<BehaviorEntry> {
        let ts_s: String = row.get("ts")?;
        let kind_s: String = row.get("kind")?;
        let map = |e: CoreError| rusqlite::Error::ToSqlConversionFailure(Box::new(e));
        Ok(BehaviorEntry {
            id: Some(row.get("id")?),
            ts: parse_ts(&ts_s).map_err(map)?,
            kind: kind_s.parse().map_err(map)?,
            content: row.get("content")?,
            source: row.get("source")?,
        })
    }

    /// The full journal, newest first.
    pub fn list_behavior(&self) -> Result<Vec<BehaviorEntry>> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM behavior_log ORDER BY ts DESC, id DESC")?;
        let rows = stmt.query_map([], Self::row_to_behavior)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Journal entries of one kind within `[from, to]`, oldest first (the shape
    /// habit detection wants).
    pub fn list_behavior_between(
        &self,
        kind: BehaviorKind,
        from: NaiveDateTime,
        to: NaiveDateTime,
    ) -> Result<Vec<BehaviorEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT * FROM behavior_log WHERE kind = ?1 AND ts >= ?2 AND ts <= ?3 ORDER BY ts ASC, id ASC",
        )?;
        let rows = stmt.query_map(
            params![kind.as_str(), fmt_ts(&from), fmt_ts(&to)],
            Self::row_to_behavior,
        )?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// The most recent timestamp of a given kind (e.g. the last check-in ask).
    pub fn last_behavior_ts(&self, kind: BehaviorKind) -> Result<Option<NaiveDateTime>> {
        let s: Option<String> = self
            .conn
            .query_row(
                "SELECT ts FROM behavior_log WHERE kind = ?1 ORDER BY ts DESC, id DESC LIMIT 1",
                params![kind.as_str()],
                |r| r.get(0),
            )
            .optional()?;
        s.map(|s| parse_ts(&s)).transpose()
    }

    pub fn delete_behavior(&self, id: i64) -> Result<()> {
        let n = self
            .conn
            .execute("DELETE FROM behavior_log WHERE id = ?1", params![id])?;
        if n == 0 {
            return Err(CoreError::NotFound(format!("behavior#{id}")));
        }
        Ok(())
    }

    // ---- suggestions (F10) --------------------------------------------------

    /// Insert a suggestion unless one with the same `dedup_key` already exists.
    /// Returns the new id, or `None` if it was a duplicate.
    pub fn insert_suggestion_if_new(&self, s: &Suggestion) -> Result<Option<i64>> {
        let n = self.conn.execute(
            "INSERT OR IGNORE INTO suggestions(created_at, kind, text, dedup_key, source, status, guid)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                fmt_ts(&s.created_at),
                s.kind.as_str(),
                s.text,
                s.dedup_key,
                s.source,
                s.status.as_str(),
                new_guid(),
            ],
        )?;
        Ok((n > 0).then(|| self.conn.last_insert_rowid()))
    }

    pub(crate) fn row_to_suggestion(row: &rusqlite::Row) -> rusqlite::Result<Suggestion> {
        let created_s: String = row.get("created_at")?;
        let kind_s: String = row.get("kind")?;
        let status_s: String = row.get("status")?;
        let map = |e: CoreError| rusqlite::Error::ToSqlConversionFailure(Box::new(e));
        Ok(Suggestion {
            id: Some(row.get("id")?),
            created_at: parse_ts(&created_s).map_err(map)?,
            kind: kind_s.parse().map_err(map)?,
            text: row.get("text")?,
            dedup_key: row.get("dedup_key")?,
            source: row.get("source")?,
            status: status_s.parse().map_err(map)?,
        })
    }

    pub fn get_suggestion(&self, id: i64) -> Result<Suggestion> {
        self.conn
            .query_row(
                "SELECT * FROM suggestions WHERE id = ?1",
                params![id],
                Self::row_to_suggestion,
            )
            .optional()?
            .ok_or_else(|| CoreError::NotFound(format!("suggestion#{id}")))
    }

    /// All suggestions, newest first.
    pub fn list_suggestions(&self) -> Result<Vec<Suggestion>> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM suggestions ORDER BY created_at DESC, id DESC")?;
        let rows = stmt.query_map([], Self::row_to_suggestion)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Decide a suggestion — `pending` → accepted/dismissed, once.
    ///
    /// The `WHERE status = 'pending'` is the whole point and must stay: without
    /// it a stale card (an old window, a second UI, a replayed request) could
    /// flip an already-`dismissed` suggestion back to `accepted`, and accepting
    /// has side effects — it creates a routine, or pauses one. The user's "no"
    /// has to be final.
    ///
    /// Returns `Ok(false)` when the suggestion exists but was already decided,
    /// so callers can say so instead of silently re-running the side effect.
    pub fn decide_suggestion(&self, id: i64, status: SuggestionStatus) -> Result<bool> {
        let n = self.conn.execute(
            "UPDATE suggestions SET status = ?1 WHERE id = ?2 AND status = 'pending'",
            params![status.as_str(), id],
        )?;
        if n > 0 {
            return Ok(true);
        }
        // Distinguish "no such suggestion" from "already decided".
        let exists: i64 = self.conn.query_row(
            "SELECT count(*) FROM suggestions WHERE id = ?1",
            params![id],
            |r| r.get(0),
        )?;
        if exists == 0 {
            return Err(CoreError::NotFound(format!("suggestion#{id}")));
        }
        Ok(false)
    }

    pub fn delete_suggestion(&self, id: i64) -> Result<()> {
        let n = self
            .conn
            .execute("DELETE FROM suggestions WHERE id = ?1", params![id])?;
        if n == 0 {
            return Err(CoreError::NotFound(format!("suggestion#{id}")));
        }
        Ok(())
    }

    // ---- wearable health samples (F5, Phase 4) -------------------------------

    /// Insert a sample, or update the one already stored for the same
    /// `dedup_key` (a re-poll of an overlapping platform time window). Returns
    /// the new id, or `None` when an existing row was refreshed instead.
    ///
    /// `dedup_key` deliberately excludes the value: "the same kind of reading,
    /// over the same interval, from the same source" is *one* measurement, and
    /// re-polling must not stack up copies of it. But that made `INSERT OR
    /// IGNORE` wrong — a platform that later corrects a reading re-sends the
    /// same key with a new value, and ignoring it meant Solum kept the first
    /// number forever, contradicting `dedup_key`'s own doc comment. Upsert the
    /// value so a correction lands.
    pub fn insert_health_sample_if_new(&self, s: &HealthSample) -> Result<Option<i64>> {
        let n = self.conn.execute(
            "INSERT INTO health_samples(kind, start, end, value, source, created_at, dedup_key, guid)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(dedup_key) DO UPDATE SET value = excluded.value
               WHERE health_samples.value <> excluded.value",
            params![
                s.kind.as_str(),
                fmt_ts(&s.start),
                fmt_ts(&s.end),
                s.value,
                s.source,
                fmt_ts(&s.created_at),
                s.dedup_key(),
                new_guid(),
            ],
        )?;
        Ok((n > 0).then(|| self.conn.last_insert_rowid()))
    }

    pub(crate) fn row_to_health_sample(row: &rusqlite::Row) -> rusqlite::Result<HealthSample> {
        let kind_s: String = row.get("kind")?;
        let start_s: String = row.get("start")?;
        let end_s: String = row.get("end")?;
        let created_s: String = row.get("created_at")?;
        let map = |e: CoreError| rusqlite::Error::ToSqlConversionFailure(Box::new(e));
        Ok(HealthSample {
            id: Some(row.get("id")?),
            kind: kind_s.parse().map_err(map)?,
            start: parse_ts(&start_s).map_err(map)?,
            end: parse_ts(&end_s).map_err(map)?,
            value: row.get("value")?,
            source: row.get("source")?,
            created_at: parse_ts(&created_s).map_err(map)?,
        })
    }

    /// All samples, newest first (the F12 ledger's shape).
    pub fn list_health_samples(&self) -> Result<Vec<HealthSample>> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM health_samples ORDER BY start DESC, id DESC")?;
        let rows = stmt.query_map([], Self::row_to_health_sample)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Samples of one kind within `[from, to]`, oldest first.
    pub fn list_health_samples_between(
        &self,
        kind: HealthMetric,
        from: NaiveDateTime,
        to: NaiveDateTime,
    ) -> Result<Vec<HealthSample>> {
        let mut stmt = self.conn.prepare(
            "SELECT * FROM health_samples WHERE kind = ?1 AND start >= ?2 AND start <= ?3 ORDER BY start ASC, id ASC",
        )?;
        let rows = stmt.query_map(
            params![kind.as_str(), fmt_ts(&from), fmt_ts(&to)],
            Self::row_to_health_sample,
        )?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn delete_health_sample(&self, id: i64) -> Result<()> {
        let n = self
            .conn
            .execute("DELETE FROM health_samples WHERE id = ?1", params![id])?;
        if n == 0 {
            return Err(CoreError::NotFound(format!("health_sample#{id}")));
        }
        Ok(())
    }

    // ---- Soulous read-only facts (Phase 8.1) --------------------------------

    pub(crate) fn row_to_soulous_fact(row: &rusqlite::Row) -> rusqlite::Result<SoulousFact> {
        let kind_s: String = row.get("kind")?;
        let occurs_s: Option<String> = row.get("occurs_at")?;
        let ends_s: Option<String> = row.get("ends_at")?;
        let imported_s: String = row.get("imported_at")?;
        let map = |e: CoreError| rusqlite::Error::ToSqlConversionFailure(Box::new(e));
        Ok(SoulousFact {
            id: Some(row.get("id")?),
            external_id: row.get("external_id")?,
            kind: kind_s.parse().map_err(map)?,
            title: row.get("title")?,
            occurs_at: occurs_s.map(|s| parse_ts(&s)).transpose().map_err(map)?,
            ends_at: ends_s.map(|s| parse_ts(&s)).transpose().map_err(map)?,
            payload_json: row.get("payload_json")?,
            source: row.get("source")?,
            imported_at: parse_ts(&imported_s).map_err(map)?,
        })
    }

    /// The complete cached snapshot, newest data first. These rows are never
    /// added to `memory_ledger` or recall: they are a visible data source,
    /// not something Solum inferred or remembered.
    pub fn list_soulous_facts(&self) -> Result<Vec<SoulousFact>> {
        let mut stmt = self.conn.prepare(
            "SELECT * FROM soulous_facts ORDER BY occurs_at DESC, imported_at DESC, id DESC",
        )?;
        let rows = stmt.query_map([], Self::row_to_soulous_fact)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Atomically replace the source snapshot only after every remote endpoint
    /// has parsed successfully. Trigger-based sync records row upserts and
    /// removals; a network/parser failure occurs before this transaction.
    pub fn replace_soulous_snapshot(&self, facts: &[SoulousFact]) -> Result<()> {
        if facts.iter().any(|f| f.source != SOULOUS_SOURCE) {
            return Err(CoreError::Invalid(
                "Soulous snapshot contains a non-soulous source marker".into(),
            ));
        }
        let wanted: std::collections::HashSet<String> =
            facts.iter().map(SoulousFact::guid).collect();
        let tx = self.conn.unchecked_transaction()?;
        for fact in facts {
            tx.execute(
                "INSERT INTO soulous_facts(external_id, kind, title, occurs_at, ends_at, payload_json, source, imported_at, guid)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                 ON CONFLICT(guid) DO UPDATE SET external_id=excluded.external_id,
                    kind=excluded.kind, title=excluded.title, occurs_at=excluded.occurs_at,
                    ends_at=excluded.ends_at, payload_json=excluded.payload_json,
                    source=excluded.source, imported_at=excluded.imported_at",
                params![
                    fact.external_id,
                    fact.kind.as_str(),
                    fact.title,
                    fact.occurs_at.map(|d| fmt_ts(&d)),
                    fact.ends_at.map(|d| fmt_ts(&d)),
                    fact.payload_json,
                    fact.source,
                    fmt_ts(&fact.imported_at),
                    fact.guid(),
                ],
            )?;
        }
        let mut stmt = tx.prepare("SELECT guid FROM soulous_facts")?;
        let existing: Vec<String> = stmt
            .query_map([], |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(stmt);
        for guid in existing {
            if !wanted.contains(&guid) {
                tx.execute("DELETE FROM soulous_facts WHERE guid = ?1", params![guid])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    // ---- semantic memory facts (§3.10) --------------------------------------

    /// Insert a fact unless the identical content is already remembered.
    /// Returns the new id, or `None` for a duplicate.
    pub fn insert_fact_if_new(&self, f: &crate::memory::MemoryFact) -> Result<Option<i64>> {
        let n = self.conn.execute(
            "INSERT OR IGNORE INTO memory_facts(content, source, created_at, last_used_at, guid)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                f.content,
                f.source,
                fmt_ts(&f.created_at),
                f.last_used_at.map(|d| fmt_ts(&d)),
                new_guid(),
            ],
        )?;
        Ok((n > 0).then(|| self.conn.last_insert_rowid()))
    }

    pub(crate) fn row_to_fact(row: &rusqlite::Row) -> rusqlite::Result<crate::memory::MemoryFact> {
        let created_s: String = row.get("created_at")?;
        let used_s: Option<String> = row.get("last_used_at")?;
        let map = |e: CoreError| rusqlite::Error::ToSqlConversionFailure(Box::new(e));
        Ok(crate::memory::MemoryFact {
            id: Some(row.get("id")?),
            content: row.get("content")?,
            source: row.get("source")?,
            created_at: parse_ts(&created_s).map_err(map)?,
            last_used_at: match used_s {
                Some(s) => Some(parse_ts(&s).map_err(map)?),
                None => None,
            },
        })
    }

    /// All facts, newest first.
    pub fn list_facts(&self) -> Result<Vec<crate::memory::MemoryFact>> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM memory_facts ORDER BY created_at DESC, id DESC")?;
        let rows = stmt.query_map([], Self::row_to_fact)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn delete_fact(&self, id: i64) -> Result<()> {
        let n = self
            .conn
            .execute("DELETE FROM memory_facts WHERE id = ?1", params![id])?;
        if n == 0 {
            return Err(CoreError::NotFound(format!("fact#{id}")));
        }
        Ok(())
    }

    /// Edit a fact's wording in place (F12: 记忆可编辑). Recall reads this
    /// table directly, so the new wording takes effect immediately.
    /// UNIQUE(content) still holds — editing into another fact's exact
    /// wording is rejected rather than silently merged.
    pub fn update_fact(&self, id: i64, content: &str) -> Result<()> {
        let content = content.trim();
        if content.is_empty() {
            return Err(CoreError::Invalid("记忆内容不能为空".into()));
        }
        let n = self
            .conn
            .execute(
                "UPDATE memory_facts SET content = ?1 WHERE id = ?2",
                params![content, id],
            )
            .map_err(|e| match e {
                rusqlite::Error::SqliteFailure(f, _)
                    if f.code == rusqlite::ErrorCode::ConstraintViolation =>
                {
                    CoreError::Invalid("已有一条相同内容的记忆".into())
                }
                other => other.into(),
            })?;
        if n == 0 {
            return Err(CoreError::NotFound(format!("fact#{id}")));
        }
        Ok(())
    }

    // ---- routines (F3 完全体, D4) --------------------------------------------

    /// Insert a routine unless an *active* one with the same title already
    /// exists (double-accepting a habit suggestion must not double-remind).
    pub fn insert_routine_if_new(&self, r: &crate::routine::Routine) -> Result<Option<i64>> {
        let existing: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM routines WHERE title = ?1 AND active = 1",
                params![r.title],
                |row| row.get(0),
            )
            .optional()?;
        if existing.is_some() {
            return Ok(None);
        }
        self.conn.execute(
            "INSERT INTO routines(title, time_of_day, source, active, created_at, scheduled_until, guid)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                r.title,
                r.time_of_day,
                r.source,
                r.active as i64,
                fmt_ts(&r.created_at),
                r.scheduled_until.map(|d| d.format("%Y-%m-%d").to_string()),
                new_guid(),
            ],
        )?;
        Ok(Some(self.conn.last_insert_rowid()))
    }

    pub(crate) fn row_to_routine(row: &rusqlite::Row) -> rusqlite::Result<crate::routine::Routine> {
        let created_s: String = row.get("created_at")?;
        let until_s: Option<String> = row.get("scheduled_until")?;
        let active: i64 = row.get("active")?;
        let map = |e: CoreError| rusqlite::Error::ToSqlConversionFailure(Box::new(e));
        Ok(crate::routine::Routine {
            id: Some(row.get("id")?),
            title: row.get("title")?,
            time_of_day: row.get("time_of_day")?,
            source: row.get("source")?,
            active: active != 0,
            created_at: parse_ts(&created_s).map_err(map)?,
            scheduled_until: until_s
                .and_then(|s| chrono::NaiveDate::parse_from_str(&s, "%Y-%m-%d").ok()),
        })
    }

    /// All routines, newest first.
    pub fn list_routines(&self) -> Result<Vec<crate::routine::Routine>> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM routines ORDER BY created_at DESC, id DESC")?;
        let rows = stmt.query_map([], Self::row_to_routine)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn set_routine_active(&self, id: i64, active: bool) -> Result<()> {
        let n = self.conn.execute(
            "UPDATE routines
             SET active = ?1,
                 scheduled_until = CASE
                     WHEN active = 0 AND ?1 = 1 THEN NULL
                     ELSE scheduled_until
                 END
             WHERE id = ?2",
            params![active as i64, id],
        )?;
        if n == 0 {
            return Err(CoreError::NotFound(format!("routine#{id}")));
        }
        if !active {
            self.remove_pending_routine_occurrences(id)?;
        }
        Ok(())
    }

    /// Change an existing routine's mutable configuration. Past occurrences
    /// are historical records, while pending ones are projections of this
    /// configuration and must be rebuilt from the new title/time.
    pub fn update_routine(&self, id: i64, title: &str, time_of_day: &str) -> Result<()> {
        let title = title.trim();
        if title.is_empty() {
            return Err(CoreError::Invalid("固定提醒内容不能为空".into()));
        }
        let time = NaiveTime::parse_from_str(time_of_day, "%H:%M").map_err(|e| {
            CoreError::Invalid(format!("固定提醒时间 {:?} 不合法: {e}", time_of_day))
        })?;
        let time_of_day = time.format("%H:%M").to_string();

        let active: Option<i64> = self
            .conn
            .query_row(
                "SELECT active FROM routines WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .optional()?;
        let active = active.ok_or_else(|| CoreError::NotFound(format!("routine#{id}")))?;
        if active != 0 {
            let duplicate: Option<i64> = self
                .conn
                .query_row(
                    "SELECT id FROM routines WHERE title = ?1 AND active = 1 AND id != ?2 LIMIT 1",
                    params![title, id],
                    |row| row.get(0),
                )
                .optional()?;
            if let Some(existing) = duplicate {
                return Err(CoreError::Invalid(format!(
                    "已存在启用的同名固定提醒「{title}」（routine#{existing}）"
                )));
            }
        }

        self.conn.execute(
            "UPDATE routines SET title = ?1, time_of_day = ?2, scheduled_until = NULL WHERE id = ?3",
            params![title, time_of_day, id],
        )?;
        self.remove_pending_routine_occurrences(id)?;
        Ok(())
    }

    /// Advance the materialization high-water mark.
    pub fn set_routine_scheduled_until(&self, id: i64, until: chrono::NaiveDate) -> Result<()> {
        let n = self.conn.execute(
            "UPDATE routines SET scheduled_until = ?1 WHERE id = ?2",
            params![until.format("%Y-%m-%d").to_string(), id],
        )?;
        if n == 0 {
            return Err(CoreError::NotFound(format!("routine#{id}")));
        }
        Ok(())
    }

    pub fn delete_routine(&self, id: i64) -> Result<()> {
        self.with_transaction(|s| {
            s.remove_pending_routine_occurrences(id)?;
            let n = s
                .conn
                .execute("DELETE FROM routines WHERE id = ?1", params![id])?;
            if n == 0 {
                return Err(CoreError::NotFound(format!("routine#{id}")));
            }
            Ok(())
        })
    }

    /// Retract materialized occurrences that have not fully fired yet. The
    /// event carries `routine_id` explicitly, rather than relying on its
    /// display title, so pausing one routine cannot touch a similarly named
    /// user-created event.
    pub(crate) fn remove_pending_routine_occurrences(&self, routine_id: i64) -> Result<usize> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT e.id FROM events e
             JOIN notifications n ON n.event_id = e.id
             WHERE e.routine_id = ?1 AND n.status = 'pending'",
        )?;
        let ids: Vec<i64> = stmt
            .query_map(params![routine_id], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for event_id in &ids {
            self.delete_event(*event_id)?;
        }
        Ok(ids.len())
    }

    /// Whether an event with exactly this title and start already exists —
    /// keeps routine materialization idempotent across devices (post-sync).
    pub fn event_exists(&self, title: &str, start: NaiveDateTime) -> Result<bool> {
        let hit: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM events WHERE title = ?1 AND start = ?2 LIMIT 1",
                params![title, fmt_ts(&start)],
                |r| r.get(0),
            )
            .optional()?;
        Ok(hit.is_some())
    }


}
