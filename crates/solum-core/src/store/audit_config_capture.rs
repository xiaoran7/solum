use super::*;

impl Store {
    // ---- audit log (append-only) -----------------------------------------

    pub fn append_audit(&self, e: &AuditEntry) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO audit_log(ts, tool, risk, summary, decision, token_id, detail)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                fmt_ts(&e.ts),
                e.tool,
                e.risk.as_str(),
                e.summary,
                e.decision.as_str(),
                e.token_id,
                e.detail,
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn list_audit(&self) -> Result<Vec<AuditRow>> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM audit_log ORDER BY id ASC")?;
        let rows = stmt.query_map([], |row| {
            Ok(AuditRow {
                id: row.get("id")?,
                ts: row.get("ts")?,
                tool: row.get("tool")?,
                risk: row.get("risk")?,
                summary: row.get("summary")?,
                decision: row.get("decision")?,
                token_id: row.get("token_id")?,
                detail: row.get("detail")?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    // ---- config -----------------------------------------------------------

    pub(crate) fn get_meta(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT value FROM meta WHERE key = ?1", params![key], |r| {
                r.get(0)
            })
            .optional()?)
    }

    pub(crate) fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO meta(key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn load_rule_table(&self) -> Result<RuleTable> {
        match self.get_meta("rule_table")? {
            Some(json) => RuleTable::from_json(&json),
            None => Ok(RuleTable::default_table()),
        }
    }

    pub fn save_rule_table(&self, t: &RuleTable) -> Result<()> {
        self.set_meta("rule_table", &t.to_json()?)
    }

    pub fn load_proactivity(&self) -> Result<ProactivityConfig> {
        match self.get_meta("proactivity")? {
            Some(json) => ProactivityConfig::from_json(&json),
            None => Ok(ProactivityConfig::defaults()),
        }
    }

    pub fn save_proactivity(&self, c: &ProactivityConfig) -> Result<()> {
        self.set_meta("proactivity", &c.to_json()?)
    }

    /// Whether subsequently captured third-party notifications may leave the
    /// device through the existing sync / recall-context paths. Missing means
    /// disabled: notification text needs an explicit opt-in before it may be
    /// sent to a cloud model. A saved choice still wins on upgrades.
    pub fn notif_cloud_enabled(&self) -> Result<bool> {
        match self.get_meta(NOTIF_CLOUD_META_KEY)? {
            None => Ok(false),
            Some(value) => match value.as_str() {
                "true" => Ok(true),
                "false" => Ok(false),
                _ => Err(CoreError::Invalid(format!(
                    "{} 必须是 true 或 false",
                    NOTIF_CLOUD_META_KEY
                ))),
            },
        }
    }

    /// Persist this device's capture-time notification privacy choice. It is
    /// intentionally not a synced meta key: another device must not silently
    /// alter this device's outbound-data preference.
    pub fn set_notif_cloud_enabled(&self, enabled: bool) -> Result<()> {
        self.set_meta(NOTIF_CLOUD_META_KEY, if enabled { "true" } else { "false" })
    }

    /// F20's package whitelist and confirmed local filtering rules. This is
    /// device-local like the notification-cloud toggle: installed package ids
    /// and capture consent must not be silently changed by another device.
    pub fn notification_intelligence_config(&self) -> Result<NotificationIntelligenceConfig> {
        match self.get_meta(NOTIFICATION_INTELLIGENCE_META_KEY)? {
            Some(json) => serde_json::from_str::<NotificationIntelligenceConfig>(&json)
                .map_err(Into::into)
                .and_then(NotificationIntelligenceConfig::normalized),
            None => Ok(NotificationIntelligenceConfig::default()),
        }
    }

    pub fn save_notification_intelligence_config(
        &self,
        config: &NotificationIntelligenceConfig,
    ) -> Result<()> {
        let config = config.clone().normalized()?;
        self.set_meta(
            NOTIFICATION_INTELLIGENCE_META_KEY,
            &serde_json::to_string(&config)?,
        )
    }

    // ---- F20 notification capture journal ---------------------------------

    pub fn insert_notification_capture(
        &self,
        capture: &NotificationCapture,
        local_only: bool,
        lane: CaptureLane,
        state: CaptureState,
        reason: Option<&str>,
    ) -> Result<NotificationCaptureRecord> {
        let content_hash =
            crate::notification_intelligence::content_hash(&capture.package_name, &capture.text());
        let raw_input_id = self.insert_raw_input_with_scope(
            &capture.raw_input(),
            "notification_capture",
            capture.received_at,
            local_only,
            "notification",
            Some(&content_hash),
        )?;
        self.conn.execute(
            "INSERT INTO notification_captures(raw_input_id, package_name, title, body, received_at, content_hash, local_only, lane, state, reason, event_id, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, NULL, ?11)",
            params![
                raw_input_id,
                capture.package_name,
                capture.title,
                capture.body,
                fmt_ts(&capture.received_at),
                content_hash,
                local_only as i64,
                lane.as_str(),
                state.as_str(),
                reason,
                fmt_ts(&capture.received_at),
            ],
        )?;
        Ok(NotificationCaptureRecord {
            id: Some(self.conn.last_insert_rowid()),
            raw_input_id,
            package_name: capture.package_name.clone(),
            title: capture.title.clone(),
            body: capture.body.clone(),
            received_at: capture.received_at,
            content_hash,
            local_only,
            lane,
            state,
            reason: reason.map(ToOwned::to_owned),
            event_id: None,
            created_at: capture.received_at,
        })
    }

    pub(crate) fn row_to_notification_capture(
        row: &rusqlite::Row,
    ) -> rusqlite::Result<NotificationCaptureRecord> {
        pub(crate) fn from_core<T>(value: crate::error::Result<T>) -> rusqlite::Result<T> {
            value.map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })
        }
        let received_at: String = row.get("received_at")?;
        let created_at: String = row.get("created_at")?;
        let lane: String = row.get("lane")?;
        let state: String = row.get("state")?;
        Ok(NotificationCaptureRecord {
            id: Some(row.get("id")?),
            raw_input_id: row.get("raw_input_id")?,
            package_name: row.get("package_name")?,
            title: row.get("title")?,
            body: row.get("body")?,
            received_at: from_core(parse_ts(&received_at))?,
            content_hash: row.get("content_hash")?,
            local_only: row.get::<_, i64>("local_only")? != 0,
            lane: from_core(lane.parse::<CaptureLane>())?,
            state: from_core(state.parse::<CaptureState>())?,
            reason: row.get("reason")?,
            event_id: row.get("event_id")?,
            created_at: from_core(parse_ts(&created_at))?,
        })
    }

    pub fn notification_capture(&self, id: i64) -> Result<NotificationCaptureRecord> {
        self.conn
            .query_row(
                "SELECT * FROM notification_captures WHERE id = ?1",
                params![id],
                Self::row_to_notification_capture,
            )
            .optional()?
            .ok_or_else(|| CoreError::NotFound(format!("notification_capture#{id}")))
    }

    pub fn list_notification_captures(&self) -> Result<Vec<NotificationCaptureRecord>> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM notification_captures ORDER BY received_at DESC, id DESC")?;
        let rows = stmt.query_map([], Self::row_to_notification_capture)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn queued_notification_captures(
        &self,
        lane: Option<CaptureLane>,
    ) -> Result<Vec<NotificationCaptureRecord>> {
        let sql = if lane.is_some() {
            "SELECT * FROM notification_captures WHERE state = 'queued' AND lane = ?1 ORDER BY received_at ASC, id ASC"
        } else {
            "SELECT * FROM notification_captures WHERE state = 'queued' ORDER BY received_at ASC, id ASC"
        };
        let mut stmt = self.conn.prepare(sql)?;
        let rows = match lane {
            Some(lane) => {
                stmt.query_map(params![lane.as_str()], Self::row_to_notification_capture)?
            }
            None => stmt.query_map([], Self::row_to_notification_capture)?,
        };
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn has_recent_notification_duplicate(
        &self,
        package_name: &str,
        content_hash: &str,
        since: NaiveDateTime,
    ) -> Result<bool> {
        Ok(self
            .conn
            .query_row(
                "SELECT 1 FROM notification_captures
                 WHERE package_name = ?1 AND content_hash = ?2 AND received_at >= ?3 LIMIT 1",
                params![package_name, content_hash, fmt_ts(&since)],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    pub fn set_notification_capture_state(
        &self,
        id: i64,
        state: CaptureState,
        reason: Option<&str>,
        event_id: Option<i64>,
    ) -> Result<()> {
        let changed = self.conn.execute(
            "UPDATE notification_captures SET state = ?1, reason = ?2, event_id = ?3 WHERE id = ?4",
            params![state.as_str(), reason, event_id, id],
        )?;
        if changed == 0 {
            return Err(CoreError::NotFound(format!("notification_capture#{id}")));
        }
        Ok(())
    }

    pub fn insert_notification_filter_proposal(
        &self,
        proposal: &NotificationFilterProposal,
    ) -> Result<NotificationFilterProposal> {
        self.conn.execute(
            "INSERT INTO notification_filter_proposals(package_name, pattern, matcher, reason, state, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                proposal.package_name,
                proposal.pattern,
                match proposal.matcher {
                    crate::classify::NotificationMatchKind::Substring => "substring",
                    crate::classify::NotificationMatchKind::Regex => "regex",
                },
                proposal.reason,
                proposal.state.as_str(),
                fmt_ts(&proposal.created_at),
            ],
        )?;
        let mut out = proposal.clone();
        out.id = Some(self.conn.last_insert_rowid());
        Ok(out)
    }

    pub(crate) fn row_to_notification_filter_proposal(
        row: &rusqlite::Row,
    ) -> rusqlite::Result<NotificationFilterProposal> {
        let matcher: String = row.get("matcher")?;
        let state: String = row.get("state")?;
        let created_at: String = row.get("created_at")?;
        let matcher = match matcher.as_str() {
            "substring" => crate::classify::NotificationMatchKind::Substring,
            "regex" => crate::classify::NotificationMatchKind::Regex,
            _other => {
                return Err(rusqlite::Error::InvalidColumnType(
                    0,
                    "matcher".into(),
                    rusqlite::types::Type::Text,
                ))
            }
        };
        let state = state.parse().map_err(|error: CoreError| {
            rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?;
        Ok(NotificationFilterProposal {
            id: Some(row.get("id")?),
            package_name: row.get("package_name")?,
            pattern: row.get("pattern")?,
            matcher,
            reason: row.get("reason")?,
            state,
            created_at: parse_ts(&created_at).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })?,
        })
    }

    pub fn list_notification_filter_proposals(&self) -> Result<Vec<NotificationFilterProposal>> {
        let mut stmt = self.conn.prepare(
            "SELECT * FROM notification_filter_proposals ORDER BY created_at DESC, id DESC",
        )?;
        let rows = stmt.query_map([], Self::row_to_notification_filter_proposal)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn set_notification_filter_proposal_state(
        &self,
        id: i64,
        state: FilterProposalState,
    ) -> Result<NotificationFilterProposal> {
        let changed = self.conn.execute(
            "UPDATE notification_filter_proposals SET state = ?1 WHERE id = ?2",
            params![state.as_str(), id],
        )?;
        if changed == 0 {
            return Err(CoreError::NotFound(format!(
                "notification_filter_proposal#{id}"
            )));
        }
        self.conn
            .query_row(
                "SELECT * FROM notification_filter_proposals WHERE id = ?1",
                params![id],
                Self::row_to_notification_filter_proposal,
            )
            .map_err(Into::into)
    }

    pub fn insert_notification_action_proposal(
        &self,
        proposal: &NotificationActionProposal,
    ) -> Result<NotificationActionProposal> {
        self.conn.execute(
            "INSERT INTO notification_action_proposals(capture_id, kind, event_id, event_title, event_guid, event_start, new_start, reason, state, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                proposal.capture_id,
                proposal.kind.as_str(),
                proposal.event_id,
                proposal.event_title,
                proposal.event_guid,
                fmt_ts(&proposal.event_start),
                proposal.new_start.map(|value| fmt_ts(&value)),
                proposal.reason,
                proposal.state.as_str(),
                fmt_ts(&proposal.created_at),
            ],
        )?;
        let mut out = proposal.clone();
        out.id = Some(self.conn.last_insert_rowid());
        Ok(out)
    }

    pub(crate) fn row_to_notification_action_proposal(
        row: &rusqlite::Row,
    ) -> rusqlite::Result<NotificationActionProposal> {
        pub(crate) fn from_core<T>(value: crate::error::Result<T>) -> rusqlite::Result<T> {
            value.map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })
        }
        let kind: String = row.get("kind")?;
        let state: String = row.get("state")?;
        let new_start: Option<String> = row.get("new_start")?;
        let created_at: String = row.get("created_at")?;
        Ok(NotificationActionProposal {
            id: Some(row.get("id")?),
            capture_id: row.get("capture_id")?,
            kind: from_core(kind.parse::<NotificationActionKind>())?,
            event_id: row.get("event_id")?,
            event_title: row.get("event_title")?,
            event_guid: row.get("event_guid")?,
            // Rows written before the snapshot columns existed carry an empty
            // string; `parse_ts` would fail, so map it to the epoch and let the
            // guid check (also empty → mismatch) do the refusing.
            event_start: match row.get::<_, String>("event_start")? {
                s if s.is_empty() => NaiveDateTime::default(),
                s => from_core(parse_ts(&s))?,
            },
            new_start: new_start
                .map(|value| from_core(parse_ts(&value)))
                .transpose()?,
            reason: row.get("reason")?,
            state: from_core(state.parse::<ActionProposalState>())?,
            created_at: from_core(parse_ts(&created_at))?,
        })
    }

    pub fn list_notification_action_proposals(&self) -> Result<Vec<NotificationActionProposal>> {
        let mut stmt = self.conn.prepare(
            "SELECT * FROM notification_action_proposals ORDER BY created_at DESC, id DESC",
        )?;
        let rows = stmt.query_map([], Self::row_to_notification_action_proposal)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn notification_action_proposal(&self, id: i64) -> Result<NotificationActionProposal> {
        self.conn
            .query_row(
                "SELECT * FROM notification_action_proposals WHERE id = ?1",
                params![id],
                Self::row_to_notification_action_proposal,
            )
            .optional()?
            .ok_or_else(|| CoreError::NotFound(format!("notification_action_proposal#{id}")))
    }

    pub fn set_notification_action_proposal_state(
        &self,
        id: i64,
        state: ActionProposalState,
    ) -> Result<()> {
        let changed = self.conn.execute(
            "UPDATE notification_action_proposals SET state = ?1 WHERE id = ?2",
            params![state.as_str(), id],
        )?;
        if changed == 0 {
            return Err(CoreError::NotFound(format!(
                "notification_action_proposal#{id}"
            )));
        }
        Ok(())
    }

    pub(crate) fn delete_notification_capture(&self, id: i64) -> Result<()> {
        let raw_input_id: i64 = self
            .conn
            .query_row(
                "SELECT raw_input_id FROM notification_captures WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .optional()?
            .ok_or_else(|| CoreError::NotFound(format!("notification_capture#{id}")))?;
        self.conn.execute(
            "DELETE FROM notification_action_proposals WHERE capture_id = ?1",
            params![id],
        )?;
        self.conn.execute(
            "DELETE FROM notification_captures WHERE id = ?1",
            params![id],
        )?;
        self.delete_raw_input_cascade(raw_input_id)
    }

    /// Erase an input and everything derived from it, as one unit.
    ///
    /// This is the user's right-to-delete path: "forget what I said" has to
    /// mean the events, reminders, captures and proposals that were built out
    /// of it too. Half of that is arguably worse than none — a deletion that
    /// reports success while leaving the derived copies behind is a privacy
    /// promise the data doesn't keep.
    pub(crate) fn delete_raw_input_cascade(&self, id: i64) -> Result<()> {
        self.with_transaction(|s| {
            // Cascade: events derived from this input, and their notifications.
            let ids: Vec<i64> = {
                let mut stmt = s
                    .conn
                    .prepare("SELECT id FROM events WHERE raw_input_id = ?1")?;
                let rows = stmt
                    .query_map(params![id], |row| row.get(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                rows
            };
            for event_id in ids {
                s.delete_event(event_id)?;
            }
            s.conn.execute(
                "DELETE FROM notification_action_proposals
                 WHERE capture_id IN (SELECT id FROM notification_captures WHERE raw_input_id = ?1)",
                params![id],
            )?;
            s.conn.execute(
                "DELETE FROM notification_captures WHERE raw_input_id = ?1",
                params![id],
            )?;
            let changed = s
                .conn
                .execute("DELETE FROM raw_inputs WHERE id = ?1", params![id])?;
            if changed == 0 {
                return Err(CoreError::NotFound(format!("raw_input#{id}")));
            }
            Ok(())
        })
    }
}
