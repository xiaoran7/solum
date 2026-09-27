//! `apply` -- the per-op dispatch: one arm per synced table, each writing that table's row. Deliberately one function — it is a single match whose arms share the payload-parsing closures above them, and splitting it would mean extracting arms into helpers, which is a change to the code rather than a move of it.
//!
//! Part of the `sync` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::*;
impl Store {
    /// Apply a single remote op. Returns false when the op is skipped (orphan
    /// FK, duplicate content, unresolvable pointer) — that is data-dependent,
    /// not an error.
    pub(crate) fn apply_one(&self, op: &crate::sync::SyncOp) -> Result<bool> {
        if op.op == "delete" {
            return self.apply_delete(op);
        }
        let payload: serde_json::Value = match &op.payload {
            Some(p) => serde_json::from_str(p)?,
            None => return Ok(false),
        };
        let s = |k: &str| payload.get(k).and_then(|v| v.as_str()).map(str::to_string);
        let need = |k: &str| {
            s(k).ok_or_else(|| {
                CoreError::Invalid(format!("sync op {}:{} missing {k}", op.tbl, op.guid))
            })
        };
        match op.tbl.as_str() {
            "raw_inputs" => {
                let text = need("text")?;
                // Prefer the originating device's capture-time decision so one
                // record has the same cloud-LLM eligibility everywhere. Older
                // payloads predate the field; fall back to the conservative
                // notification-prefix guess rather than defaulting to allowed.
                let local_only = payload
                    .get("local_only")
                    .and_then(serde_json::Value::as_i64)
                    .map_or_else(|| text.starts_with("[通知·"), |v| v != 0);
                let source_id = s("source_id").unwrap_or_else(|| {
                    if text.starts_with("[通知·") {
                        "notification".into()
                    } else {
                        "manual".into()
                    }
                });
                let external_id = s("external_id");
                self.conn.execute(
                "INSERT INTO raw_inputs(text, intent, created_at, local_only, source_id, external_id, guid) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(guid) DO UPDATE SET text=excluded.text, intent=excluded.intent,
                   created_at=excluded.created_at, local_only=excluded.local_only,
                   source_id=excluded.source_id, external_id=excluded.external_id",
                params![
                    text,
                    need("intent")?,
                    need("created_at")?,
                    local_only as i64,
                    source_id,
                    external_id,
                    op.guid
                ],
            )?;
            }
            "events" => {
                let raw_id: Option<i64> = match s("raw_input_guid") {
                    Some(g) => self.id_by_guid("raw_inputs", &g)?,
                    None => None,
                };
                let routine_id: Option<i64> = match s("routine_guid") {
                    Some(g) => self.id_by_guid("routines", &g)?,
                    None => None,
                };
                // Same rule as raw_inputs: the transmitted stamp wins, and a
                // payload without it falls back to the parent raw input.
                let local_only = match payload
                    .get("local_only")
                    .and_then(serde_json::Value::as_i64)
                {
                    Some(v) => v != 0,
                    None => raw_id
                        .map(|id| self.raw_input_is_local_only(id))
                        .transpose()?
                        .unwrap_or(false),
                };
                self.conn.execute(
                "INSERT INTO events(title, kind, start, end, location, people_json, raw_input_id, routine_id, created_at, local_only, guid)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                 ON CONFLICT(guid) DO UPDATE SET title=excluded.title, kind=excluded.kind,
                   start=excluded.start, end=excluded.end, location=excluded.location,
                   people_json=excluded.people_json, raw_input_id=excluded.raw_input_id,
                   routine_id=excluded.routine_id, created_at=excluded.created_at,
                   local_only=excluded.local_only",
                params![
                    need("title")?,
                    need("kind")?,
                    need("start")?,
                    s("end"),
                    s("location"),
                    need("people_json")?,
                    raw_id,
                    routine_id,
                    need("created_at")?,
                    local_only as i64,
                    op.guid
                ],
            )?;
            }
            "notifications" => {
                // Orphan (event deleted or not yet arrived): skip. The event's
                // own op precedes its notifications in origin oplog order, so
                // in-order delivery only orphans genuinely deleted parents.
                let Some(event_id) = s("event_guid")
                    .map(|g| self.id_by_guid("events", &g))
                    .transpose()?
                    .flatten()
                else {
                    return Ok(false);
                };
                let local_only = self.event_is_local_only(event_id)?;
                self.conn.execute(
                "INSERT INTO notifications(event_id, fire_at, lead_label, channels_json, status, created_at, fired_at, local_only, guid)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                 ON CONFLICT(guid) DO UPDATE SET event_id=excluded.event_id, fire_at=excluded.fire_at,
                   lead_label=excluded.lead_label, channels_json=excluded.channels_json,
                   status=excluded.status, created_at=excluded.created_at, fired_at=excluded.fired_at,
                   local_only=excluded.local_only",
                params![
                    event_id,
                    need("fire_at")?,
                    need("lead_label")?,
                    need("channels_json")?,
                    need("status")?,
                    need("created_at")?,
                    s("fired_at"),
                    local_only as i64,
                    op.guid
                ],
            )?;
            }
            "behavior_log" => {
                self.conn.execute(
                "INSERT INTO behavior_log(ts, kind, content, source, guid) VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(guid) DO UPDATE SET ts=excluded.ts, kind=excluded.kind,
                   content=excluded.content, source=excluded.source",
                params![need("ts")?, need("kind")?, need("content")?, s("source"), op.guid],
            )?;
            }
            "suggestions" => {
                // Both devices can generate the same suggestion independently
                // (same dedup_key, different guid) — treat that as a skip.
                let r = self.conn.execute(
                "INSERT INTO suggestions(created_at, kind, text, dedup_key, source, status, guid)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(guid) DO UPDATE SET created_at=excluded.created_at, kind=excluded.kind,
                   text=excluded.text, dedup_key=excluded.dedup_key, source=excluded.source,
                   status=excluded.status",
                params![
                    need("created_at")?,
                    need("kind")?,
                    need("text")?,
                    need("dedup_key")?,
                    s("source"),
                    need("status")?,
                    op.guid
                ],
            );
                if let Err(rusqlite::Error::SqliteFailure(e, _)) = &r {
                    if e.code == rusqlite::ErrorCode::ConstraintViolation {
                        return Ok(false);
                    }
                }
                r?;
            }
            "persona_versions" => {
                // Rows are immutable history: same guid → already have it.
                if self.id_by_guid("persona_versions", &op.guid)?.is_some() {
                    return Ok(false);
                }
                // Version numbers are device-local; renumber on collision and
                // keep the embedded JSON consistent.
                let wanted = payload.get("version").and_then(|v| v.as_i64()).unwrap_or(0);
                let taken: bool = self
                    .conn
                    .query_row(
                        "SELECT 1 FROM persona_versions WHERE version = ?1",
                        params![wanted],
                        |_| Ok(()),
                    )
                    .optional()?
                    .is_some();
                let version = if wanted > 0 && !taken {
                    wanted
                } else {
                    self.conn.query_row(
                        "SELECT COALESCE(MAX(version), 0) + 1 FROM persona_versions",
                        [],
                        |r| r.get(0),
                    )?
                };
                let mut profile =
                    crate::persona::PersonaProfile::from_json(&need("profile_json")?)?;
                profile.version = version;
                self.conn.execute(
                "INSERT INTO persona_versions(version, created_at, profile_json, guid) VALUES (?1, ?2, ?3, ?4)",
                params![version, need("created_at")?, profile.to_json()?, op.guid],
            )?;
            }
            "health_samples" => {
                // Same "independently produced, same content, different
                // guid" tolerance as suggestions: skip on dedup_key clash.
                let r = self.conn.execute(
                "INSERT INTO health_samples(kind, start, end, value, source, created_at, dedup_key, guid)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT(guid) DO UPDATE SET kind=excluded.kind, start=excluded.start,
                   end=excluded.end, value=excluded.value, source=excluded.source,
                   created_at=excluded.created_at, dedup_key=excluded.dedup_key",
                params![
                    need("kind")?,
                    need("start")?,
                    need("end")?,
                    payload.get("value").and_then(|v| v.as_f64()).unwrap_or(0.0),
                    need("source")?,
                    need("created_at")?,
                    need("dedup_key")?,
                    op.guid
                ],
            );
                if let Err(rusqlite::Error::SqliteFailure(e, _)) = &r {
                    if e.code == rusqlite::ErrorCode::ConstraintViolation {
                        return Ok(false);
                    }
                }
                r?;
            }
            "memory_facts" => {
                // Same tolerance as suggestions: the same fact written
                // independently on two devices (UNIQUE content) is a skip.
                let r = self.conn.execute(
                    "INSERT INTO memory_facts(content, source, created_at, last_used_at, guid)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(guid) DO UPDATE SET content=excluded.content,
                   source=excluded.source, created_at=excluded.created_at,
                   last_used_at=excluded.last_used_at",
                    params![
                        need("content")?,
                        need("source")?,
                        need("created_at")?,
                        s("last_used_at"),
                        op.guid
                    ],
                );
                if let Err(rusqlite::Error::SqliteFailure(e, _)) = &r {
                    if e.code == rusqlite::ErrorCode::ConstraintViolation {
                        return Ok(false);
                    }
                }
                r?;
            }
            "routines" => {
                let active = payload.get("active").and_then(|v| v.as_i64()).unwrap_or(1);
                let title = need("title")?.to_string();
                let time_of_day = need("time_of_day")?.to_string();
                // A remote configuration edit must retract this device's
                // local pending projections before the next ticker rebuilds
                // them. Fired occurrences remain historical records.
                let previous: Option<(String, String, i64)> = self
                    .conn
                    .query_row(
                        "SELECT title, time_of_day, active FROM routines WHERE guid = ?1",
                        params![op.guid],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                    )
                    .optional()?;
                let rebuild_pending = active == 0
                    || previous.is_some_and(|(old_title, old_time, old_active)| {
                        old_active == 0 || old_title != title || old_time != time_of_day
                    });
                self.conn.execute(
                "INSERT INTO routines(title, time_of_day, source, active, created_at, scheduled_until, guid)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(guid) DO UPDATE SET title=excluded.title,
                   time_of_day=excluded.time_of_day, source=excluded.source,
                   active=excluded.active, created_at=excluded.created_at,
                   scheduled_until=excluded.scheduled_until",
                params![
                    title,
                    time_of_day,
                    s("source"),
                    active,
                    need("created_at")?,
                    s("scheduled_until"),
                    op.guid
                ],
            )?;
                if rebuild_pending {
                    if let Some(routine_id) = self.id_by_guid("routines", &op.guid)? {
                        self.remove_pending_routine_occurrences(routine_id)?;
                    }
                }
            }
            "soulous_facts" => {
                let source = need("source")?;
                if source != SOULOUS_SOURCE {
                    return Err(CoreError::Invalid(
                        "refusing a non-soulous row in soulous_facts sync".into(),
                    ));
                }
                self.conn.execute(
                "INSERT INTO soulous_facts(external_id, kind, title, occurs_at, ends_at, payload_json, source, imported_at, guid)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                 ON CONFLICT(guid) DO UPDATE SET external_id=excluded.external_id,
                   kind=excluded.kind, title=excluded.title, occurs_at=excluded.occurs_at,
                   ends_at=excluded.ends_at, payload_json=excluded.payload_json,
                   source=excluded.source, imported_at=excluded.imported_at",
                params![
                    need("external_id")?,
                    need("kind")?,
                    need("title")?,
                    s("occurs_at"),
                    s("ends_at"),
                    need("payload_json")?,
                    source,
                    need("imported_at")?,
                    op.guid
                ],
            )?;
            }
            "widget_defs" => {
                self.conn.execute(
                    "INSERT INTO widget_defs(name, icon, list_sort_by, table_sort_by,
                   created_at, guid)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(guid) DO UPDATE SET name=excluded.name, icon=excluded.icon,
                   list_sort_by=excluded.list_sort_by, table_sort_by=excluded.table_sort_by,
                   created_at=excluded.created_at",
                    params![
                        need("name")?,
                        need("icon")?,
                        s("list_sort_by"),
                        s("table_sort_by"),
                        need("created_at")?,
                        op.guid
                    ],
                )?;
            }
            "widget_fields" => {
                // Orphan: the parent definition was deleted, or has not
                // arrived yet. Same rule as notifications — skip rather than
                // invent a parent.
                let Some(widget_id) = self.id_by_guid("widget_defs", &need("widget_guid")?)? else {
                    return Ok(false);
                };
                let name = need("name")?;
                // The one case a grow-only set does not resolve by itself: two
                // devices independently added the same field name to the same
                // widget, so the union contains a duplicate. Keep the smaller
                // guid — deterministic on every device without consulting the
                // clock — and log the loser instead of dropping it silently
                // (the quarantine round's rule: losing data is survivable,
                // losing it quietly is not).
                let existing: Option<String> = self
                    .conn
                    .query_row(
                        "SELECT guid FROM widget_fields WHERE widget_id = ?1 AND name = ?2",
                        params![widget_id, name],
                        |row| row.get(0),
                    )
                    .optional()?;
                if let Some(existing_guid) = existing {
                    if existing_guid != op.guid {
                        if existing_guid <= op.guid {
                            self.append_widget_schema_rejection(
                                op.payload.as_deref().unwrap_or("{}"),
                                &format!(
                                "同名字段 {name:?} 在另一台设备上并发创建，保留 guid 较小的一份"
                            ),
                                parse_ts(&need("created_at")?).unwrap_or_default(),
                            )?;
                            return Ok(false);
                        }
                        // The arriving row wins; retire the local duplicate so
                        // the unique index does not reject the upsert.
                        self.conn.execute(
                            "DELETE FROM widget_fields WHERE widget_id = ?1 AND name = ?2",
                            params![widget_id, name],
                        )?;
                    }
                }
                self.conn.execute(
                    "INSERT INTO widget_fields(widget_id, name, label, field_type, required,
                   options_json, ord, form_ord, list_ord, table_ord, stat_ord,
                   created_at, guid)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
                 ON CONFLICT(guid) DO UPDATE SET widget_id=excluded.widget_id,
                   name=excluded.name, label=excluded.label, field_type=excluded.field_type,
                   required=excluded.required, options_json=excluded.options_json,
                   ord=excluded.ord, form_ord=excluded.form_ord, list_ord=excluded.list_ord,
                   table_ord=excluded.table_ord, stat_ord=excluded.stat_ord,
                   created_at=excluded.created_at",
                    params![
                        widget_id,
                        name,
                        need("label")?,
                        need("field_type")?,
                        payload
                            .get("required")
                            .and_then(serde_json::Value::as_i64)
                            .unwrap_or(0),
                        need("options_json")?,
                        payload
                            .get("ord")
                            .and_then(serde_json::Value::as_i64)
                            .unwrap_or(0),
                        payload.get("form_ord").and_then(serde_json::Value::as_i64),
                        payload.get("list_ord").and_then(serde_json::Value::as_i64),
                        payload.get("table_ord").and_then(serde_json::Value::as_i64),
                        payload.get("stat_ord").and_then(serde_json::Value::as_i64),
                        need("created_at")?,
                        op.guid
                    ],
                )?;
            }
            "widget_records" => {
                let Some(widget_id) = self.id_by_guid("widget_defs", &need("widget_guid")?)? else {
                    return Ok(false);
                };
                self.conn.execute(
                    "INSERT INTO widget_records(widget_id, data_json, created_at, guid)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(guid) DO UPDATE SET widget_id=excluded.widget_id,
                   data_json=excluded.data_json, created_at=excluded.created_at",
                    params![widget_id, need("data_json")?, need("created_at")?, op.guid],
                )?;
            }
            "meta" => {
                let key = need("key")?;
                let mut value = need("value")?;
                if key == "persona_active" {
                    // Pointer travels as a version-row guid; resolve locally.
                    let Some(version) = self
                        .conn
                        .query_row(
                            "SELECT version FROM persona_versions WHERE guid = ?1",
                            params![value],
                            |r| r.get::<_, i64>(0),
                        )
                        .optional()?
                    else {
                        return Ok(false);
                    };
                    value = version.to_string();
                }
                self.set_meta(&key, &value)?;
            }
            other => {
                return Err(CoreError::Invalid(format!("unknown sync table {other}")));
            }
        }
        Ok(true)
    }
}
