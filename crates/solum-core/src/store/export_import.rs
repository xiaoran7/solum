use super::*;

impl Store {
    // ---- restorable export / import -------------------------------------

    /// Every syncable row in wire shape: `{guid, ...payload}` per table.
    ///
    /// The human-readable layers of an export are not restorable — they carry
    /// no guid, so re-importing one would duplicate everything instead of
    /// converging, and FK links (an event's raw input) would be unrecoverable.
    /// This section exists so a backup can actually be replayed.
    pub fn export_restore_rows(&self) -> Result<serde_json::Map<String, serde_json::Value>> {
        let mut out = serde_json::Map::new();
        for (tbl, payload, syncable) in SYNC_PAYLOADS {
            // The very expressions the capture triggers use, read directly off
            // the table instead of off `NEW`.
            let expr = payload.replace("NEW.", "");
            let cond = syncable.replace("NEW.", "");
            let sql = format!("SELECT guid, {expr} FROM {tbl} WHERE guid IS NOT NULL AND ({cond})");
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            let mut list = Vec::new();
            for row in rows {
                let (guid, payload_json) = row?;
                let mut value: serde_json::Value = serde_json::from_str(&payload_json)?;
                if let Some(object) = value.as_object_mut() {
                    object.insert("guid".into(), serde_json::Value::String(guid));
                }
                list.push(value);
            }
            out.insert((*tbl).to_string(), serde_json::Value::Array(list));
        }
        // Config that syncs as LWW meta documents rides along too, or a
        // restored device would come back without its rule table.
        let mut metas = Vec::new();
        let mut stmt = self.conn.prepare(&format!(
            "SELECT key, value FROM meta WHERE key IN ({SYNCED_META_KEYS})"
        ))?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (key, value) = row?;
            metas.push(
                serde_json::json!({ "guid": format!("meta:{key}"), "key": key, "value": value }),
            );
        }
        out.insert("meta".into(), serde_json::Value::Array(metas));
        Ok(out)
    }

    /// Replay a `_restore` section through the ordinary merge path.
    ///
    /// Stamping every row with the export's own timestamp (not "now") is the
    /// load-bearing choice: LWW then treats a restored row exactly like a row
    /// arriving late from another device, so **restoring an old backup cannot
    /// clobber newer local edits**, and importing the same file twice is a
    /// no-op. A restore is not privileged over what the user has since done.
    pub fn import_restore_rows(
        &self,
        restore: &serde_json::Value,
        exported_at: NaiveDateTime,
        origin: &str,
        now: NaiveDateTime,
    ) -> Result<MergeCounts> {
        let object = restore
            .as_object()
            .ok_or_else(|| CoreError::Invalid("_restore 必须是对象".into()))?;
        // A backup file is untrusted input — it can be edited, or come from
        // somewhere the user did not expect. `exported_at` is only the file's
        // own claim about when it was written, and it is used directly as the
        // LWW stamp: a document claiming the year 3000 beats every local row,
        // now and forever, which makes "import never destroys local data" a
        // false promise.
        //
        // **Refuse** such a document rather than silently repairing it.
        //
        // Clamping the stamp down to "now" was the first instinct and it is
        // not enough: the content would then be old but stamped fresh, which
        // still beats every edit the user made before the import. The file is
        // either an honest backup — in which case its timestamp is in the past
        // and nothing needs fixing — or it is not one, and the honest response
        // is to say so rather than to guess at a repair.
        //
        // The comparison uses **the clock the competing stamps use**: every
        // local write is stamped by the sync trigger's `strftime('now')`, i.e.
        // real UTC. The injected `now` is a local naive time and may even be
        // the debug simulated clock, so the more permissive of the two is the
        // ceiling — this check is about catching forgery, not about policing
        // ordinary timezone and clock skew.
        let db_now: String =
            self.conn
                .query_row("SELECT strftime('%Y-%m-%dT%H:%M:%f', 'now')", [], |r| {
                    r.get(0)
                })?;
        let hlc = exported_at.format("%Y-%m-%dT%H:%M:%S%.3f").to_string();
        let ceiling = now
            .max(parse_ts(&db_now).unwrap_or(now))
            .checked_add_signed(chrono::Duration::days(1))
            .unwrap_or(now);
        if exported_at > ceiling {
            return Err(CoreError::Invalid(format!(
                "备份自称的导出时间（{}）在未来，已拒绝导入。\
                 这份文件要么被改动过，要么来自时钟错误的设备；\
                 直接合并会让它永久压过你本机后续的所有修改。",
                exported_at.format("%Y-%m-%d %H:%M")
            )));
        }

        let mut total_rows = 0usize;
        let mut ops = Vec::new();
        for (tbl, rows) in object {
            // Reject unknown tables rather than parking them.
            //
            // Quarantine exists for ops from a *peer running a newer build* —
            // data we should keep because a later version will understand it.
            // A table name in an imported file is not that: it is whatever the
            // file said. Letting it through meant a crafted backup could fill
            // the 5 000-op quarantine and push out real cross-device data that
            // was genuinely waiting for an upgrade.
            // The authority is what `export_restore_rows` actually writes:
            // every `SYNC_PAYLOADS` table, plus `meta` (config rows that sync
            // as LWW documents — without them a restored device comes back
            // missing its rule table).
            let known = SYNC_PAYLOADS.iter().any(|(name, _, _)| name == tbl) || tbl == "meta";
            if !known {
                return Err(CoreError::Invalid(format!(
                    "备份里含有本机不认识的表 `{tbl}`，已拒绝导入（备份可能被改动过或来自其他程序）"
                )));
            }
            let rows = rows
                .as_array()
                .ok_or_else(|| CoreError::Invalid(format!("_restore.{tbl} 必须是数组")))?;
            total_rows += rows.len();
            if total_rows > MAX_IMPORT_ROWS {
                return Err(CoreError::Invalid(format!(
                    "备份超过 {MAX_IMPORT_ROWS} 行上限，已拒绝导入"
                )));
            }
            for row in rows {
                let guid = row
                    .get("guid")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| CoreError::Invalid(format!("_restore.{tbl} 有行缺少 guid")))?;
                let payload = row.to_string();
                if payload.len() > MAX_IMPORT_ROW_BYTES {
                    return Err(CoreError::Invalid(format!(
                        "_restore.{tbl} 有单行超过 {MAX_IMPORT_ROW_BYTES} 字节上限，已拒绝导入"
                    )));
                }
                ops.push(crate::sync::SyncOp {
                    tbl: tbl.clone(),
                    guid: guid.to_string(),
                    op: "upsert".into(),
                    payload: Some(payload),
                    hlc: hlc.clone(),
                    origin: origin.to_string(),
                });
            }
        }
        // Parents before children, so an event's raw input already exists when
        // the event's own FK-by-guid lookup runs.
        let order = |tbl: &str| {
            SYNC_PAYLOADS
                .iter()
                .position(|(name, _, _)| *name == tbl)
                .unwrap_or(usize::MAX)
        };
        ops.sort_by_key(|op| order(&op.tbl));
        self.apply_remote_ops(&ops)
    }

    /// Refuse merge-style v3 recovery. The platform coordinator must provide a
    /// newly created temporary Store, never a live profile with user data.
    pub(crate) fn ensure_empty_for_portable_restore(&self) -> Result<()> {
        const USER_TABLES: &[&str] = &[
            "raw_inputs",
            "events",
            "notifications",
            "behavior_log",
            "suggestions",
            "persona_versions",
            "health_samples",
            "memory_facts",
            "routines",
            "soulous_facts",
            "notification_captures",
            "notification_filter_proposals",
            "notification_action_proposals",
            "widget_defs",
            "widget_fields",
            "widget_records",
            "widget_schema_rejections",
            "audit_log",
            "sync_oplog",
            "sync_quarantine",
            "capture_loss_receipts",
            "sync_bad_blobs",
            "sync_push_holds",
        ];
        for table in USER_TABLES {
            let sql = format!("SELECT EXISTS(SELECT 1 FROM {table} LIMIT 1)");
            if self.conn.query_row(&sql, [], |row| row.get::<_, bool>(0))? {
                return Err(CoreError::Invalid(
                    "portable restore requires a new empty temporary Store".into(),
                ));
            }
        }
        let has_non_bootstrap_meta = self.conn.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM meta
                WHERE key NOT IN ('device_id', 'schema_version')
            )",
            [],
            |row| row.get::<_, bool>(0),
        )?;
        if has_non_bootstrap_meta {
            return Err(CoreError::Invalid(
                "portable restore requires a new empty temporary Store".into(),
            ));
        }
        Ok(())
    }

    /// Restore v3's non-guid SQLite ledgers after `_restore` has populated the
    /// guid-bearing parents. The caller owns the encompassing transaction.
    pub(crate) fn import_portable_section(
        &self,
        section: &crate::export_v3::PortableSection,
    ) -> Result<()> {
        use crate::notification_intelligence::{
            NotificationFilterRule, NotificationIntelligenceConfig,
        };
        use std::collections::BTreeMap;

        let mut capture_ids = BTreeMap::new();
        for row in &section.notification_journal.captures {
            let (raw_input_id, raw_local_only): (i64, bool) = self
                .conn
                .query_row(
                    "SELECT id, local_only FROM raw_inputs WHERE guid = ?1",
                    params![row.raw_input_guid],
                    |record| Ok((record.get(0)?, record.get::<_, i64>(1)? != 0)),
                )
                .optional()?
                .ok_or_else(|| {
                    CoreError::Invalid(format!(
                        "portable capture {} has no restored raw input",
                        row.id
                    ))
                })?;
            if raw_local_only != row.local_only {
                return Err(CoreError::Invalid(format!(
                    "portable capture {} privacy scope does not match its raw input",
                    row.id
                )));
            }
            let event_id = row
                .event_guid
                .as_deref()
                .map(|guid| {
                    self.conn
                        .query_row(
                            "SELECT id FROM events WHERE guid = ?1",
                            params![guid],
                            |r| r.get::<_, i64>(0),
                        )
                        .optional()
                        .map_err(CoreError::from)
                })
                .transpose()?
                .flatten();
            self.conn.execute(
                "INSERT INTO notification_captures(
                    raw_input_id, package_name, title, body, received_at,
                    content_hash, local_only, lane, state, reason, event_id, created_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                params![
                    raw_input_id,
                    row.package_name,
                    row.title,
                    row.body,
                    row.received_at,
                    row.content_hash,
                    row.local_only as i64,
                    row.lane,
                    row.restore_state.as_str(),
                    row.reason,
                    event_id,
                    row.created_at,
                ],
            )?;
            capture_ids.insert(row.id.as_str(), self.conn.last_insert_rowid());
        }

        for row in &section.notification_journal.filter_proposals {
            let matcher = match row.matcher {
                crate::classify::NotificationMatchKind::Substring => "substring",
                crate::classify::NotificationMatchKind::Regex => "regex",
            };
            self.conn.execute(
                "INSERT INTO notification_filter_proposals(
                    package_name, pattern, matcher, reason, state, created_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    row.package_name,
                    row.pattern,
                    matcher,
                    row.reason,
                    row.state,
                    row.created_at,
                ],
            )?;
        }

        for row in &section.notification_journal.action_proposals {
            let capture_id = capture_ids.get(row.capture_id.as_str()).ok_or_else(|| {
                CoreError::Invalid(format!(
                    "portable action {} has no restored capture",
                    row.id
                ))
            })?;
            let resolved_event = row
                .event_guid
                .as_deref()
                .map(|guid| {
                    self.conn
                        .query_row(
                            "SELECT id FROM events WHERE guid = ?1",
                            params![guid],
                            |r| r.get::<_, i64>(0),
                        )
                        .optional()
                        .map_err(CoreError::from)
                })
                .transpose()?
                .flatten();
            self.conn.execute(
                "INSERT INTO notification_action_proposals(
                    capture_id, kind, event_id, event_title, event_guid,
                    event_start, new_start, reason, state, created_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    capture_id,
                    row.kind.as_str(),
                    resolved_event.unwrap_or_default(),
                    row.event_title,
                    row.event_guid.as_deref().unwrap_or_default(),
                    row.event_start,
                    row.new_start,
                    row.reason,
                    row.restore_state.as_str(),
                    row.created_at,
                ],
            )?;
        }

        for row in &section.audit_log {
            self.conn.execute(
                "INSERT INTO audit_log(ts, tool, risk, summary, decision, token_id, detail)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    row.ts,
                    row.tool,
                    row.risk,
                    row.summary,
                    row.decision,
                    row.token_id,
                    row.detail,
                ],
            )?;
        }

        let notification = &section.agent_settings.notification;
        self.save_notification_intelligence_config(&NotificationIntelligenceConfig {
            capture_mode: crate::notification_intelligence::NotificationCaptureMode::All,
            allowed_packages: Vec::new(),
            auto_event_packages: Vec::new(),
            batch_interval_minutes: notification.batch_interval_minutes,
            filter_rules: notification
                .filter_rules
                .iter()
                .map(|row| NotificationFilterRule {
                    id: row.id.clone(),
                    pattern: row.pattern.clone(),
                    package_name: row.package_name.clone(),
                    matcher: row.matcher,
                    reason: row.reason.clone(),
                })
                .collect(),
        })?;
        Ok(())
    }

    /// How many rows a `_restore` section would offer, per table — the
    /// Guard preview needs this before anything is written.
    pub fn count_restore_rows(restore: &serde_json::Value) -> Vec<(String, usize)> {
        let Some(object) = restore.as_object() else {
            return Vec::new();
        };
        let mut counts: Vec<(String, usize)> = object
            .iter()
            .filter_map(|(tbl, rows)| {
                let len = rows.as_array()?.len();
                (len > 0).then(|| (tbl.clone(), len))
            })
            .collect();
        counts.sort();
        counts
    }

    pub fn count_widget_definitions(&self) -> Result<i64> {
        Ok(self
            .conn
            .query_row("SELECT COUNT(*) FROM widget_defs", [], |row| row.get(0))?)
    }

    pub fn get_widget_definition(&self, id: i64) -> Result<WidgetDefinition> {
        // Two steps because assembling the schema needs its own query, which
        // cannot run while the outer statement is still borrowed.
        struct Head {
            name: String,
            icon: String,
            list_sort_by: Option<String>,
            table_sort_by: Option<String>,
            created_at: String,
        }
        let head: Option<Head> = self
            .conn
            .query_row(
                "SELECT name, icon, list_sort_by, table_sort_by, created_at
                 FROM widget_defs WHERE id = ?1",
                params![id],
                |row| {
                    Ok(Head {
                        name: row.get(0)?,
                        icon: row.get(1)?,
                        list_sort_by: row.get(2)?,
                        table_sort_by: row.get(3)?,
                        created_at: row.get(4)?,
                    })
                },
            )
            .optional()?;
        let Head {
            name,
            icon,
            list_sort_by,
            table_sort_by,
            created_at,
        } = head.ok_or_else(|| CoreError::NotFound(format!("widget#{id}")))?;
        Ok(WidgetDefinition {
            id,
            name,
            icon,
            schema: self.load_widget_schema(id, list_sort_by, table_sort_by)?,
            created_at: parse_ts(&created_at)?,
        })
    }

    pub fn list_widget_definitions(&self) -> Result<Vec<WidgetDefinition>> {
        let ids: Vec<i64> = {
            let mut stmt = self
                .conn
                .prepare("SELECT id FROM widget_defs ORDER BY created_at DESC, id DESC")?;
            let rows = stmt.query_map([], |row| row.get(0))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        ids.into_iter()
            .map(|id| self.get_widget_definition(id))
            .collect()
    }

    pub(crate) fn row_to_widget_record(row: &rusqlite::Row) -> rusqlite::Result<WidgetRecord> {
        let data_json: String = row.get("data_json")?;
        let created_at: String = row.get("created_at")?;
        let data = serde_json::from_str(&data_json).map_err(|e| {
            rusqlite::Error::ToSqlConversionFailure(Box::new(CoreError::Invalid(format!(
                "widget record JSON 损坏：{e}"
            ))))
        })?;
        let map = |e: CoreError| rusqlite::Error::ToSqlConversionFailure(Box::new(e));
        Ok(WidgetRecord {
            id: row.get("id")?,
            widget_id: row.get("widget_id")?,
            data,
            created_at: parse_ts(&created_at).map_err(map)?,
        })
    }

    pub fn list_widget_records(&self, widget_id: i64) -> Result<Vec<WidgetRecord>> {
        // Loading the definition first distinguishes an empty widget from an
        // invalid id, and validates the persisted schema before its records
        // reach a renderer.
        self.get_widget_definition(widget_id)?;
        let mut stmt = self.conn.prepare(
            "SELECT * FROM widget_records WHERE widget_id = ?1 ORDER BY created_at DESC, id DESC",
        )?;
        let rows = stmt.query_map(params![widget_id], Self::row_to_widget_record)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Every record across every definition, for the export document. Widgets
    /// never sync (§3.12), so this export is the only copy of hand-entered
    /// widget data that can leave the device.
    pub fn list_all_widget_records(&self) -> Result<Vec<WidgetRecord>> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM widget_records ORDER BY widget_id, created_at, id")?;
        let rows = stmt.query_map([], Self::row_to_widget_record)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn insert_widget_record(
        &self,
        widget_id: i64,
        data: &serde_json::Value,
        now: NaiveDateTime,
    ) -> Result<WidgetRecord> {
        let definition = self.get_widget_definition(widget_id)?;
        definition.schema.validate_record(data)?;
        let data_json = serde_json::to_string(data)?;
        self.conn.execute(
            "INSERT INTO widget_records(widget_id, data_json, created_at, guid)
             VALUES (?1, ?2, ?3, ?4)",
            params![widget_id, data_json, fmt_ts(&now), new_guid()],
        )?;
        Ok(WidgetRecord {
            id: self.conn.last_insert_rowid(),
            widget_id,
            data: data.clone(),
            created_at: now,
        })
    }

    pub fn update_widget_record(
        &self,
        widget_id: i64,
        id: i64,
        data: &serde_json::Value,
    ) -> Result<WidgetRecord> {
        let definition = self.get_widget_definition(widget_id)?;
        definition.schema.validate_record(data)?;
        let data_json = serde_json::to_string(data)?;
        let changed = self.conn.execute(
            "UPDATE widget_records SET data_json = ?1 WHERE id = ?2 AND widget_id = ?3",
            params![data_json, id, widget_id],
        )?;
        if changed == 0 {
            return Err(CoreError::NotFound(format!("widget_record#{id}")));
        }
        self.conn
            .query_row(
                "SELECT * FROM widget_records WHERE id = ?1 AND widget_id = ?2",
                params![id, widget_id],
                Self::row_to_widget_record,
            )
            .map_err(Into::into)
    }

    pub fn delete_widget_record(&self, widget_id: i64, id: i64) -> Result<()> {
        let changed = self.conn.execute(
            "DELETE FROM widget_records WHERE id = ?1 AND widget_id = ?2",
            params![id, widget_id],
        )?;
        if changed == 0 {
            return Err(CoreError::NotFound(format!("widget_record#{id}")));
        }
        Ok(())
    }

    pub fn widget_record_count(&self, widget_id: i64) -> Result<i64> {
        self.get_widget_definition(widget_id)?;
        self.conn
            .query_row(
                "SELECT COUNT(*) FROM widget_records WHERE widget_id = ?1",
                params![widget_id],
                |row| row.get(0),
            )
            .map_err(Into::into)
    }

    /// Deletes the definition and everything under it. This is intentionally
    /// only called by the `widget_delete` Dangerous Tool.
    ///
    /// The children are deleted **explicitly** rather than through the FK's
    /// `ON DELETE CASCADE`: SQLite does not fire a table's triggers for rows
    /// removed by a cascade action unless `recursive_triggers` is on, and it
    /// is not. Relying on the cascade here would delete the rows locally
    /// without ever writing their `delete` ops to the oplog, leaving every
    /// other device holding orphan fields and records forever.
    pub fn delete_widget_definition(&self, id: i64) -> Result<(String, i64)> {
        let definition = self.get_widget_definition(id)?;
        let count = self.widget_record_count(id)?;
        self.with_transaction(|s| {
            s.conn.execute(
                "DELETE FROM widget_records WHERE widget_id = ?1",
                params![id],
            )?;
            s.conn.execute(
                "DELETE FROM widget_fields WHERE widget_id = ?1",
                params![id],
            )?;
            let changed = s
                .conn
                .execute("DELETE FROM widget_defs WHERE id = ?1", params![id])?;
            if changed == 0 {
                return Err(CoreError::NotFound(format!("widget#{id}")));
            }
            Ok(())
        })?;
        Ok((definition.name, count))
    }

    pub fn append_widget_schema_rejection(
        &self,
        schema_json: &str,
        reason: &str,
        now: NaiveDateTime,
    ) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO widget_schema_rejections(schema_json, reason, created_at) VALUES (?1, ?2, ?3)",
            params![schema_json, reason, fmt_ts(&now)],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn list_widget_schema_rejections(&self) -> Result<Vec<WidgetSchemaRejection>> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM widget_schema_rejections ORDER BY created_at DESC, id DESC")?;
        let rows = stmt.query_map([], |row| {
            let created_at: String = row.get("created_at")?;
            let map = |e: CoreError| rusqlite::Error::ToSqlConversionFailure(Box::new(e));
            Ok(WidgetSchemaRejection {
                id: row.get("id")?,
                schema_json: row.get("schema_json")?,
                reason: row.get("reason")?,
                created_at: parse_ts(&created_at).map_err(map)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }


}
