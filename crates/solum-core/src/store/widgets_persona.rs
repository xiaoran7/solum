use super::*;

impl Store {
    // ---- bulk purge (D3, ledger_purge 工具的执行面) ---------------------------

    /// How many events this app has auto-created since `since`.
    ///
    /// Counted from `notification_captures` rather than from a separate tally:
    /// the capture rows *are* the record of what happened, so the count cannot
    /// drift from reality or be reset independently of it.
    pub fn count_auto_events_for_package(
        &self,
        package_name: &str,
        since: NaiveDateTime,
    ) -> Result<usize> {
        let n: i64 = self.conn.query_row(
            "SELECT count(*) FROM notification_captures
             WHERE package_name = ?1 AND state = 'event_created' AND created_at >= ?2",
            params![package_name, fmt_ts(&since)],
            |r| r.get(0),
        )?;
        Ok(n as usize)
    }

    /// Per-app auto-created event counts since `since`, most active first —
    /// the "this app created N entries" surface the user reviews.
    pub fn auto_event_counts_by_package(&self, since: NaiveDateTime) -> Result<Vec<(String, i64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT package_name, count(*) AS n FROM notification_captures
             WHERE state = 'event_created' AND created_at >= ?1
             GROUP BY package_name ORDER BY n DESC, package_name",
        )?;
        let rows = stmt.query_map(params![fmt_ts(&since)], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// The event this routine already materialized for `start`, if any.
    /// Identified by routine provenance, so an unrelated same-titled event
    /// (typically one that arrived via sync) is never mistaken for it.
    pub fn routine_occurrence_event(
        &self,
        routine_id: i64,
        start: NaiveDateTime,
    ) -> Result<Option<i64>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id FROM events WHERE routine_id = ?1 AND start = ?2",
                params![routine_id, fmt_ts(&start)],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// Does this routine occurrence still need materializing?
    ///
    /// True when there is no event for it **or** the event exists but has no
    /// notification at all — the latter being the half-written state a crash
    /// between the two inserts leaves behind. Answering "no" there is what
    /// used to make a missed reminder permanent.
    ///
    /// Deliberately counts notifications in *any* status, not just pending: an
    /// occurrence whose reminder already fired (or was cancelled by the user)
    /// has been materialized, and re-adding one would resurrect a reminder the
    /// user dismissed.
    pub fn routine_occurrence_needs_work(
        &self,
        routine_id: i64,
        start: NaiveDateTime,
    ) -> Result<bool> {
        let Some(ev_id) = self.routine_occurrence_event(routine_id, start)? else {
            return Ok(true);
        };
        let existing: i64 = self.conn.query_row(
            "SELECT count(*) FROM notifications WHERE event_id = ?1",
            params![ev_id],
            |r| r.get(0),
        )?;
        Ok(existing == 0)
    }

    /// How many rows a purge of `layer` before `before` would delete — the
    /// guard's effect preview reads this so the user confirms a real number.
    pub fn count_memory_before(&self, layer: MemoryLayer, before: NaiveDateTime) -> Result<usize> {
        let (table, ts_col) = purge_target(layer)?;
        let n: i64 = self.conn.query_row(
            &format!("SELECT COUNT(*) FROM {table} WHERE {ts_col} < ?1"),
            params![fmt_ts(&before)],
            |r| r.get(0),
        )?;
        Ok(n as usize)
    }

    /// Bulk-delete rows of `layer` older than `before`. Only reachable through
    /// the HITL guard (`ledger_purge`, risk=dangerous). Sync triggers capture
    /// every deleted row, so the purge propagates to other devices.
    pub fn purge_memory_before(&self, layer: MemoryLayer, before: NaiveDateTime) -> Result<usize> {
        let (table, ts_col) = purge_target(layer)?;
        let n = self.conn.execute(
            &format!("DELETE FROM {table} WHERE {ts_col} < ?1"),
            params![fmt_ts(&before)],
        )?;
        Ok(n)
    }

    // ---- persona (F9 v1 / F15) ----------------------------------------------

    /// Persist a new persona version (next version number, made active).
    pub fn insert_persona_version(
        &self,
        draft: &crate::persona::PersonaDraft,
        source: &str,
        note: Option<String>,
        now: NaiveDateTime,
    ) -> Result<crate::persona::PersonaProfile> {
        let next: i64 = self.conn.query_row(
            "SELECT COALESCE(MAX(version), 0) + 1 FROM persona_versions",
            [],
            |r| r.get(0),
        )?;
        let profile = crate::persona::PersonaProfile {
            version: next,
            created_at: now,
            source: source.to_string(),
            note,
            draft: draft.clone(),
        };
        self.conn.execute(
            "INSERT INTO persona_versions(version, created_at, profile_json, guid) VALUES (?1, ?2, ?3, ?4)",
            params![next, fmt_ts(&now), profile.to_json()?, new_guid()],
        )?;
        self.set_meta("persona_active", &next.to_string())?;
        Ok(profile)
    }

    pub fn get_persona_version(&self, version: i64) -> Result<crate::persona::PersonaProfile> {
        let json: Option<String> = self
            .conn
            .query_row(
                "SELECT profile_json FROM persona_versions WHERE version = ?1",
                params![version],
                |r| r.get(0),
            )
            .optional()?;
        match json {
            Some(j) => crate::persona::PersonaProfile::from_json(&j),
            None => Err(CoreError::NotFound(format!("persona v{version}"))),
        }
    }

    /// All persona versions, newest first.
    pub fn list_persona_versions(&self) -> Result<Vec<crate::persona::PersonaProfile>> {
        let mut stmt = self
            .conn
            .prepare("SELECT profile_json FROM persona_versions ORDER BY version DESC")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for json in rows {
            out.push(crate::persona::PersonaProfile::from_json(&json?)?);
        }
        Ok(out)
    }

    /// The active persona, or `None` when the user has never set one (or
    /// cleared it).
    pub fn active_persona(&self) -> Result<Option<crate::persona::PersonaProfile>> {
        let Some(v) = self.get_meta("persona_active")? else {
            return Ok(None);
        };
        let Ok(version) = v.parse::<i64>() else {
            return Ok(None);
        };
        // Tolerate a dangling pointer instead of failing. `clear_persona` is
        // now atomic, but a database that already went through the old
        // non-atomic path (or arrives half-merged from sync) must still be able
        // to start: "no persona" is a recoverable state, a startup error is not.
        match self.get_persona_version(version) {
            Ok(profile) => Ok(Some(profile)),
            Err(CoreError::NotFound(_)) => {
                self.conn
                    .execute("DELETE FROM meta WHERE key = 'persona_active'", [])?;
                Ok(None)
            }
            Err(e) => Err(e),
        }
    }

    /// Point the active marker at an existing version (F15 rollback — the
    /// history itself is never rewritten).
    pub fn set_active_persona(&self, version: i64) -> Result<crate::persona::PersonaProfile> {
        let profile = self.get_persona_version(version)?;
        self.set_meta("persona_active", &version.to_string())?;
        Ok(profile)
    }

    /// Delete every persona version and the active pointer (the user's
    /// right-to-delete extends to the persona, F12 spirit) — atomically.
    ///
    /// Order and atomicity both matter: dropping the versions first and dying
    /// before clearing `persona_active` leaves the pointer aimed at a version
    /// that no longer exists, and startup loads the active persona — so the
    /// app fails to start over a state the user can't see or repair.
    pub fn clear_persona(&self) -> Result<()> {
        self.with_transaction(|s| {
            s.conn
                .execute("DELETE FROM meta WHERE key = 'persona_active'", [])?;
            s.conn.execute("DELETE FROM persona_versions", [])?;
            Ok(())
        })
    }

    // ---- persistent widgets (F19; schema stored as rows since v13) -------

    /// Rebuild a `WidgetSchema` from its field rows. Views are derived: a
    /// field belongs to a view when that view's ord column is non-NULL, and
    /// the ord orders it. Ties break on guid so every device derives the same
    /// order from the same rows without consulting a clock.
    pub(crate) fn load_widget_schema(
        &self,
        widget_id: i64,
        list_sort_by: Option<String>,
        table_sort_by: Option<String>,
    ) -> Result<WidgetSchema> {
        let mut stmt = self.conn.prepare(
            "SELECT name, label, field_type, required, options_json, form_ord, list_ord,
                    table_ord, stat_ord
             FROM widget_fields WHERE widget_id = ?1 ORDER BY ord, guid",
        )?;
        let rows = stmt.query_map(params![widget_id], |row| {
            let options_json: String = row.get("options_json")?;
            let field_type: String = row.get("field_type")?;
            Ok((
                WidgetField {
                    name: row.get("name")?,
                    label: row.get("label")?,
                    field_type: serde_json::from_value(serde_json::Value::String(field_type))
                        .map_err(|e| {
                            rusqlite::Error::ToSqlConversionFailure(Box::new(CoreError::Invalid(
                                format!("字段类型无法识别：{e}"),
                            )))
                        })?,
                    required: row.get::<_, i64>("required")? != 0,
                    options: serde_json::from_str(&options_json).map_err(|e| {
                        rusqlite::Error::ToSqlConversionFailure(Box::new(CoreError::Invalid(
                            format!("字段 options 损坏：{e}"),
                        )))
                    })?,
                },
                [
                    row.get::<_, Option<i64>>("form_ord")?,
                    row.get::<_, Option<i64>>("list_ord")?,
                    row.get::<_, Option<i64>>("table_ord")?,
                    row.get::<_, Option<i64>>("stat_ord")?,
                ],
            ))
        })?;
        let loaded: Vec<LoadedWidgetField> = rows.collect::<rusqlite::Result<Vec<_>>>()?;

        let view_fields = |slot: usize| {
            let mut named: Vec<(i64, &str)> = loaded
                .iter()
                .filter_map(|entry| entry.1[slot].map(|ord| (ord, entry.0.name.as_str())))
                .collect();
            named.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(b.1)));
            named
                .into_iter()
                .map(|(_, name)| name.to_string())
                .collect::<Vec<_>>()
        };

        let mut views = Vec::new();
        for (slot, view_type) in WidgetViewType::ALL.iter().enumerate() {
            let fields = view_fields(slot);
            if fields.is_empty() {
                continue;
            }
            views.push(WidgetView {
                view_type: *view_type,
                fields,
                sort_by: match view_type {
                    WidgetViewType::List => list_sort_by.clone(),
                    WidgetViewType::Table => table_sort_by.clone(),
                    _ => None,
                },
            });
        }
        Ok(WidgetSchema {
            fields: loaded.into_iter().map(|entry| entry.0).collect(),
            views,
        })
    }

    pub fn insert_widget_definition(
        &self,
        draft: &WidgetDefinitionDraft,
        now: NaiveDateTime,
    ) -> Result<WidgetDefinition> {
        draft.validate()?;
        // Enforced here rather than in `validate()` because it is a property of
        // the device, not of the schema. Every caller goes through this path,
        // so no future CLI or import route can bypass it.
        let existing = self.count_widget_definitions()?;
        if existing >= crate::widget::MAX_WIDGETS as i64 {
            return Err(CoreError::Invalid(format!(
                "本机组件数已达上限 {}，请先删除不再使用的组件",
                crate::widget::MAX_WIDGETS
            )));
        }
        let schema = draft.schema();
        let sort_for = |view_type: WidgetViewType| {
            schema
                .views
                .iter()
                .find(|view| view.view_type == view_type)
                .and_then(|view| view.sort_by.clone())
        };
        self.conn.execute(
            "INSERT INTO widget_defs(name, icon, list_sort_by, table_sort_by, created_at, guid)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                draft.name,
                draft.icon,
                sort_for(WidgetViewType::List),
                sort_for(WidgetViewType::Table),
                fmt_ts(&now),
                new_guid()
            ],
        )?;
        let widget_id = self.conn.last_insert_rowid();
        let position = |view_type: WidgetViewType, name: &str| {
            schema
                .views
                .iter()
                .find(|view| view.view_type == view_type)
                .and_then(|view| view.fields.iter().position(|f| f == name))
                .map(|index| index as i64)
        };
        for (ord, field) in schema.fields.iter().enumerate() {
            self.conn.execute(
                "INSERT INTO widget_fields(widget_id, name, label, field_type, required,
                   options_json, ord, form_ord, list_ord, table_ord, stat_ord, created_at, guid)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                params![
                    widget_id,
                    field.name,
                    field.label,
                    field.field_type.as_str(),
                    field.required as i64,
                    serde_json::to_string(&field.options)?,
                    ord as i64,
                    position(WidgetViewType::Form, &field.name),
                    position(WidgetViewType::List, &field.name),
                    position(WidgetViewType::Table, &field.name),
                    position(WidgetViewType::Stat, &field.name),
                    fmt_ts(&now),
                    new_guid(),
                ],
            )?;
        }
        self.get_widget_definition(widget_id)
    }

    /// The only schema evolution this design allows (设计稿 ⑧): append a field.
    /// It must be optional — existing records cannot retroactively acquire a
    /// required value — which is exactly what makes records need no migration.
    pub fn add_widget_field(
        &self,
        widget_id: i64,
        field: &WidgetField,
        now: NaiveDateTime,
    ) -> Result<WidgetDefinition> {
        let definition = self.get_widget_definition(widget_id)?;
        if field.required {
            return Err(CoreError::Invalid(
                "新增字段必须可空：已有记录无法追溯填写必填值".into(),
            ));
        }
        field.validate_standalone()?;
        if definition
            .schema
            .fields
            .iter()
            .any(|existing| existing.name == field.name)
        {
            return Err(CoreError::Invalid(format!("字段 {:?} 已存在", field.name)));
        }
        // Checked against the current (possibly merged) field count, so a
        // widget that is already over the cap simply stops accepting new
        // fields rather than losing any.
        if definition.schema.fields.len() >= crate::widget::MAX_FIELDS {
            return Err(CoreError::Invalid(format!(
                "字段数已达上限 {}，无法再加",
                crate::widget::MAX_FIELDS
            )));
        }
        // Only extend views this widget actually has; creating one implicitly
        // would change its shape behind the user's back.
        let next_ord = |view_type: WidgetViewType| -> Option<i64> {
            definition
                .schema
                .views
                .iter()
                .find(|view| view.view_type == view_type)
                .map(|view| view.fields.len() as i64)
        };
        self.conn.execute(
            "INSERT INTO widget_fields(widget_id, name, label, field_type, required,
               options_json, ord, form_ord, list_ord, table_ord, stat_ord, created_at, guid)
             VALUES (?1, ?2, ?3, ?4, 0, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                widget_id,
                field.name,
                field.label,
                field.field_type.as_str(),
                serde_json::to_string(&field.options)?,
                definition.schema.fields.len() as i64,
                next_ord(WidgetViewType::Form),
                next_ord(WidgetViewType::List),
                next_ord(WidgetViewType::Table),
                // A new field never joins a stat view: "how many records filled
                // this in" is a meaningless zero for a column every existing
                // row leaves empty.
                None::<i64>,
                fmt_ts(&now),
                new_guid(),
            ],
        )?;
        self.get_widget_definition(widget_id)
    }
}
