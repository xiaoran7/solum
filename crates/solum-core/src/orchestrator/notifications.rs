use super::*;

impl Orchestrator {
    // ---- F20 notification intelligence ------------------------------------

    /// Device-local F20 configuration. All apps are captured by default; the
    /// user may switch to an explicit allowlist.
    pub fn notification_intelligence_config(&self) -> Result<NotificationIntelligenceConfig> {
        self.store.notification_intelligence_config()
    }

    pub fn set_notification_intelligence_config(
        &self,
        config: NotificationIntelligenceConfig,
    ) -> Result<NotificationIntelligenceConfig> {
        let config = config.normalized()?;
        self.store.save_notification_intelligence_config(&config)?;
        Ok(config)
    }

    pub fn set_notification_capture_mode(
        &self,
        mode: crate::notification_intelligence::NotificationCaptureMode,
    ) -> Result<NotificationIntelligenceConfig> {
        let mut config = self.store.notification_intelligence_config()?;
        config.capture_mode = mode;
        self.set_notification_intelligence_config(config)
    }

    /// Enable/disable one Android package. Enabling copies the documented
    /// best-effort per-app priority presets into the existing rule table, once.
    /// Disabling removes only the still-identifiable preset rows for that app;
    /// user-created rules have their own `user:notif:` ids and remain intact.
    /// Grant or revoke "this app may create calendar entries by itself".
    /// Separate from capture consent on purpose — see
    /// `NotificationIntelligenceConfig::auto_event_packages`.
    pub fn set_notification_app_auto_event(
        &mut self,
        package_name: &str,
        enabled: bool,
    ) -> Result<()> {
        let package_name = package_name.trim();
        let mut config = self.store.notification_intelligence_config()?;
        if enabled && !config.allows(package_name) {
            return Err(CoreError::Invalid(
                "请先允许读取该应用的通知，才能再授予「自动建日程」".into(),
            ));
        }
        config.auto_event_packages.retain(|p| p != package_name);
        if enabled {
            config.auto_event_packages.push(package_name.to_string());
        }
        self.store.save_notification_intelligence_config(&config)?;
        Ok(())
    }

    /// Per-app auto-created event counts over the trailing week. The
    /// after-the-fact discovery surface: it shows an app that is writing more
    /// than the user expected. It does **not** authorize anything.
    pub fn auto_event_counts(&self, now: NaiveDateTime) -> Result<Vec<(String, i64)>> {
        self.store
            .auto_event_counts_by_package(now - Duration::days(7))
    }

    pub fn set_notification_app_enabled(
        &mut self,
        package_name: &str,
        enabled: bool,
    ) -> Result<()> {
        let package_name = package_name.trim();
        if package_name.is_empty() || !package_name.contains('.') {
            return Err(CoreError::Invalid(
                "应用包名不能为空且应形如 com.example.app".into(),
            ));
        }
        let mut config = self.store.notification_intelligence_config()?;
        if enabled {
            if !config
                .allowed_packages
                .iter()
                .any(|package| package == package_name)
            {
                config.allowed_packages.push(package_name.to_string());
            }
            for rule in priority_presets(package_name) {
                if !self
                    .rule_table
                    .notification_priority_rules()
                    .iter()
                    .any(|existing| existing.id == rule.id)
                {
                    self.rule_table.add_notification_priority_rule(rule);
                }
            }
            self.store.save_rule_table(&self.rule_table)?;
        } else {
            config
                .allowed_packages
                .retain(|package| package != package_name);
            for rule in priority_presets(package_name) {
                self.rule_table.remove_notification_priority_rule(&rule.id);
            }
            self.store.save_rule_table(&self.rule_table)?;
        }
        self.store.save_notification_intelligence_config(&config)
    }

    pub fn add_notification_priority_rule(
        &mut self,
        pattern: String,
        package_name: Option<String>,
        matcher: NotificationMatchKind,
    ) -> Result<NotificationPriorityRule> {
        let pattern = pattern.trim().to_string();
        if pattern.is_empty() || pattern.chars().count() > 160 {
            return Err(CoreError::Invalid("重要通知模式必须是 1–160 个字符".into()));
        }
        if matcher == NotificationMatchKind::Regex {
            regex::RegexBuilder::new(&pattern)
                .case_insensitive(true)
                .build()
                .map_err(|error| CoreError::Invalid(format!("重要通知正则无效: {error}")))?;
        }
        let package_name = package_name
            .as_deref()
            .map(str::trim)
            .filter(|package| !package.is_empty())
            .map(ToOwned::to_owned);
        let rule = NotificationPriorityRule {
            id: format!("user:notif:{}", crate::store::new_guid()),
            pattern,
            package_name,
            priority: 1,
            matcher,
        };
        self.rule_table.add_notification_priority_rule(rule.clone());
        self.store.save_rule_table(&self.rule_table)?;
        Ok(rule)
    }

    pub fn remove_notification_priority_rule(&mut self, id: &str) -> Result<()> {
        if !self.rule_table.remove_notification_priority_rule(id) {
            return Err(CoreError::NotFound(format!(
                "notification priority rule {id}"
            )));
        }
        self.store.save_rule_table(&self.rule_table)
    }

    /// F20 intake boundary. A rejected package is not stored at all; all
    /// accepted captures get a visible row before deterministic filtering or
    /// dedup, so nothing is silently discarded.
    pub fn capture_notification(
        &mut self,
        capture: NotificationCapture,
    ) -> Result<Option<NotificationCaptureRecord>> {
        // Bound the third-party text at the intake boundary, before it is
        // hashed, stored, or shown to anything.
        let capture = capture.truncated();
        if capture.text().is_empty() {
            return Ok(None);
        }
        let config = self.store.notification_intelligence_config()?;
        let decision = decide_intake(
            &capture,
            &config,
            self.rule_table.notification_priority_rules(),
        );
        if matches!(decision, IntakeDecision::NotAllowed) {
            return Ok(None);
        }
        let hash = content_hash(&capture.package_name, &capture.text());
        let is_duplicate = self.store.has_recent_notification_duplicate(
            &capture.package_name,
            &hash,
            capture.received_at - Duration::minutes(DEDUP_WINDOW_MINUTES),
        )?;
        let (lane, state, reason) = if is_duplicate {
            (
                CaptureLane::Batch,
                CaptureState::Deduplicated,
                Some("同一应用、同一内容在 10 分钟内已捕获".to_string()),
            )
        } else {
            match decision {
                IntakeDecision::Filtered { rule_id, reason } => (
                    CaptureLane::Batch,
                    CaptureState::Filtered,
                    Some(if reason.is_empty() {
                        format!("已确认过滤规则 {rule_id}")
                    } else {
                        format!("已确认过滤规则 {rule_id}：{reason}")
                    }),
                ),
                IntakeDecision::Lane(lane) => (lane, CaptureState::Queued, None),
                IntakeDecision::NotAllowed => unreachable!("returned above"),
            }
        };
        let local_only = !self.store.notif_cloud_enabled()?;
        self.store
            .insert_notification_capture(&capture, local_only, lane, state, reason.as_deref())
            .map(Some)
    }

    /// Immediately process one urgent capture. The deterministic extractor is
    /// always first; only an ambiguous capture may use one cloud completion,
    /// and the Phase 9 cloud switch gates that completion completely.
    pub fn process_urgent_notification(
        &mut self,
        capture_id: i64,
        now: NaiveDateTime,
    ) -> Result<NotificationCaptureRecord> {
        let capture = self.store.notification_capture(capture_id)?;
        if capture.state != CaptureState::Queued || capture.lane != CaptureLane::Urgent {
            return Ok(capture);
        }
        self.process_notification_records(vec![capture], now, true)?;
        self.store.notification_capture(capture_id)
    }

    /// Run the ordinary lane once. The resident Android foreground service
    /// keeps the host alive; the shell calls this on the configurable 15–30
    /// minute cadence. Batch size is bounded to control cloud cost/context.
    pub fn process_notification_batch(&mut self, now: NaiveDateTime) -> Result<usize> {
        let queued = self
            .store
            .queued_notification_captures(Some(CaptureLane::Batch))?;
        let count = queued.len().min(24);
        self.process_notification_records(queued.into_iter().take(24).collect(), now, true)?;
        Ok(count)
    }

    fn process_notification_records(
        &mut self,
        records: Vec<NotificationCaptureRecord>,
        now: NaiveDateTime,
        allow_llm: bool,
    ) -> Result<()> {
        let mut uncertain = Vec::new();
        for record in records {
            if record.state != CaptureState::Queued {
                continue;
            }
            let text = format!("{} {}", record.title, record.body)
                .trim()
                .to_string();
            if route_intent(&text, now) == Intent::IngestEvent {
                if let Some(event) = self.extractor.extract(&text, now)? {
                    match self.auto_event_verdict(&record.package_name, now)? {
                        AutoEventVerdict::Allowed => {
                            self.persist_captured_event(record.id.unwrap_or(0), event)?;
                            continue;
                        }
                        AutoEventVerdict::NotGranted => {
                            self.store.set_notification_capture_state(
                                record.id.unwrap_or(0),
                                CaptureState::NeedsReview,
                                Some("识别到可排期内容；该应用未获「自动建日程」授权，请你确认后再建"),
                                None,
                            )?;
                            continue;
                        }
                        AutoEventVerdict::RateLimited => {
                            self.store.set_notification_capture_state(
                                record.id.unwrap_or(0),
                                CaptureState::NeedsReview,
                                Some("该应用今天自动创建的日程已达上限，其余改为待你确认"),
                                None,
                            )?;
                            continue;
                        }
                    }
                }
            }
            uncertain.push(record);
        }
        if uncertain.is_empty() {
            return Ok(());
        }
        // A capture keeps the scope stamped at intake. Re-enabling the cloud
        // switch must not backfill rows collected while it was off (PRIVACY.md
        // §2), so the per-row provenance decides — not the current global flag.
        let (mut uncertain, local_only_backlog): (Vec<_>, Vec<_>) =
            uncertain.into_iter().partition(|record| !record.local_only);
        for record in local_only_backlog {
            self.store.set_notification_capture_state(
                record.id.unwrap_or(0),
                CaptureState::NeedsReview,
                Some("仅本机规则未能确定；此通知在捕获时关闭了「通知上云」，不会发送到云端"),
                None,
            )?;
        }
        if uncertain.is_empty() {
            return Ok(());
        }
        let cloud_allowed = allow_llm && self.store.notif_cloud_enabled()?;
        if !cloud_allowed || self.reasoner.is_none() {
            for record in uncertain {
                self.store.set_notification_capture_state(
                    record.id.unwrap_or(0),
                    CaptureState::NeedsReview,
                    Some("离线规则未能确定；保留在通知回看中"),
                    None,
                )?;
            }
            return Ok(());
        }
        let mut captures: Vec<NotificationCapture> = uncertain
            .iter()
            .map(|record| NotificationCapture {
                package_name: record.package_name.clone(),
                title: record.title.clone(),
                body: record.body.clone(),
                received_at: record.received_at,
            })
            .collect();
        // Bound one request's worth of third-party text. Trim both lists to the
        // same prefix so the indices the model answers with keep pointing at
        // the records they came from; whatever is dropped stays queued.
        let fit = crate::notification_intelligence::fit_batch(&captures);
        captures.truncate(fit);
        uncertain.truncate(fit);
        let (system, user) = batch_triage_prompt(&captures, now);
        // Keep the immutable reasoner borrow scoped to this call; applying the
        // parsed decisions below mutates the local store and must not borrow
        // the orchestrator across a network request.
        let raw = match self
            .reasoner
            .as_deref()
            .expect("checked above")
            .complete(&system, &user)
        {
            Ok(raw) => raw,
            Err(_) => {
                for record in uncertain {
                    self.store.set_notification_capture_state(
                        record.id.unwrap_or(0),
                        CaptureState::NeedsReview,
                        Some("云端分诊暂不可用；离线处理未中断，等待回看"),
                        None,
                    )?;
                }
                return Ok(());
            }
        };
        let decisions = parse_batch_triage(&raw, &captures, now);
        let mut decided = std::collections::HashSet::new();
        for decision in decisions {
            match decision {
                LlmTriageDecision::Event {
                    capture_index,
                    event: _,
                } => {
                    // The model's *judgement* ("this notification is an
                    // appointment") is useful. Its *output* is not trustworthy
                    // enough to write to the calendar: the notification text it
                    // read is attacker-controlled — any whitelisted app, or one
                    // that has been compromised, can put instructions in a
                    // notification body and have them come back as a
                    // well-formed `event` that lands in the store with a
                    // reminder attached and no human in the loop.
                    //
                    // So the model routes, and the deterministic local
                    // extractor decides the content. Every field then traces
                    // back to the captured text by construction. If local
                    // extraction finds no unambiguous time, nothing is written
                    // and the capture goes to review for the user to judge —
                    // the one case where a silent write would be worst.
                    let record = &uncertain[capture_index];
                    let capture_id = record.id.unwrap_or(0);
                    let text = format!("{} {}", record.title, record.body)
                        .trim()
                        .to_string();
                    match self.extractor.extract(&text, now)? {
                        Some(event)
                            if self.auto_event_verdict(&record.package_name, now)?
                                == AutoEventVerdict::Allowed =>
                        {
                            self.persist_captured_event(capture_id, event)?;
                        }
                        Some(_) => {
                            self.store.set_notification_capture_state(
                                capture_id,
                                CaptureState::NeedsReview,
                                Some("识别到可排期内容；该应用未获「自动建日程」授权或已达当日上限，请你确认后再建"),
                                None,
                            )?;
                        }
                        None => {
                            self.store.set_notification_capture_state(
                                capture_id,
                                CaptureState::NeedsReview,
                                Some(
                                    "云端认为这像一条日程，但原文没有可确定的时间；请你确认后再建",
                                ),
                                None,
                            )?;
                        }
                    }
                    decided.insert(capture_index);
                }
                LlmTriageDecision::ProposeFilter {
                    capture_index,
                    proposal,
                } => {
                    let record = &uncertain[capture_index];
                    let proposal = self.store.insert_notification_filter_proposal(&proposal)?;
                    self.store.set_notification_capture_state(
                        record.id.unwrap_or(0),
                        CaptureState::NeedsReview,
                        Some(&format!(
                            "LLM 提议过滤规则 #{}，等待你的确认",
                            proposal.id.unwrap_or(0)
                        )),
                        None,
                    )?;
                    decided.insert(capture_index);
                }
                LlmTriageDecision::ProposeAction {
                    capture_index,
                    intent,
                } => {
                    let record = &uncertain[capture_index];
                    let capture_id = record.id.unwrap_or(0);
                    match self.propose_notification_action(capture_id, intent, now)? {
                        Some(proposal) => {
                            self.store.set_notification_capture_state(
                                capture_id,
                                CaptureState::NeedsReview,
                                Some(&format!(
                                    "LLM 提议{}「{}」，Rust 已匹配本地日程；等待你的确认",
                                    match proposal.kind {
                                        crate::notification_intelligence::NotificationActionKind::CancelEvent => "取消",
                                        crate::notification_intelligence::NotificationActionKind::RescheduleEvent => "改期",
                                    },
                                    proposal.event_title,
                                )),
                                None,
                            )?;
                        }
                        None => {
                            self.store.set_notification_capture_state(
                                capture_id,
                                CaptureState::NeedsReview,
                                Some("LLM 提议变更已有日程，但本地未能唯一匹配目标；未执行"),
                                None,
                            )?;
                        }
                    }
                    decided.insert(capture_index);
                }
                LlmTriageDecision::Keep { capture_index } => {
                    let record = &uncertain[capture_index];
                    self.store.set_notification_capture_state(
                        record.id.unwrap_or(0),
                        CaptureState::NeedsReview,
                        Some("LLM 建议保留；等待你决定是否提升为事件"),
                        None,
                    )?;
                    decided.insert(capture_index);
                }
            }
        }
        for (index, record) in uncertain.into_iter().enumerate() {
            if !decided.contains(&index) {
                self.store.set_notification_capture_state(
                    record.id.unwrap_or(0),
                    CaptureState::NeedsReview,
                    Some("分诊结果不完整；保留在通知回看中"),
                    None,
                )?;
            }
        }
        Ok(())
    }

    /// Whether this app may auto-create an event right now.
    ///
    /// Two independent gates. The **grant** is the authorization: capture
    /// consent is not write consent, so an app has to appear in
    /// `auto_event_packages` specifically. The **rate limit** is containment,
    /// not authorization — it exists so a granted app that misbehaves (or one
    /// the user misjudged) cannot fill the calendar before anyone notices.
    fn auto_event_verdict(
        &self,
        package_name: &str,
        now: NaiveDateTime,
    ) -> Result<AutoEventVerdict> {
        let config = self.store.notification_intelligence_config()?;
        if !config.allows_auto_event(package_name) {
            return Ok(AutoEventVerdict::NotGranted);
        }
        let since = now - Duration::days(1);
        let used = self
            .store
            .count_auto_events_for_package(package_name, since)?;
        if used >= crate::notification_intelligence::MAX_AUTO_EVENTS_PER_APP_PER_DAY {
            return Ok(AutoEventVerdict::RateLimited);
        }
        Ok(AutoEventVerdict::Allowed)
    }

    fn persist_captured_event(&mut self, capture_id: i64, event: Event) -> Result<()> {
        let capture = self.store.notification_capture(capture_id)?;
        let (event, _) =
            self.persist_event_with_scope(event, capture.raw_input_id, capture.local_only)?;
        self.store.set_notification_capture_state(
            capture_id,
            CaptureState::EventCreated,
            Some("已创建日程与提醒"),
            event.id,
        )
    }

    /// User-visible, safe promotion: a capture can become a new event only
    /// through the deterministic extractor; it never gives the LLM a record
    /// id or permission to alter existing data.
    pub fn promote_notification_capture(
        &mut self,
        capture_id: i64,
        now: NaiveDateTime,
    ) -> Result<String> {
        let capture = self.store.notification_capture(capture_id)?;
        if capture.state == CaptureState::Resolved {
            return Err(CoreError::Invalid(
                "这条通知已处理完成，不能再提升为日程".into(),
            ));
        }
        let text = format!("{} {}", capture.title, capture.body)
            .trim()
            .to_string();
        let event = self.extractor.extract(&text, now)?.ok_or_else(|| {
            CoreError::Invalid("这条通知没有可确定的时间，不能直接提升为日程".into())
        })?;
        self.persist_captured_event(capture_id, event)?;
        Ok(format!("已将通知 #{capture_id} 提升为日程。"))
    }

    pub fn restore_notification_capture(&self, capture_id: i64) -> Result<()> {
        let capture = self.store.notification_capture(capture_id)?;
        if !matches!(
            capture.state,
            CaptureState::Filtered | CaptureState::Deduplicated | CaptureState::NeedsReview
        ) {
            return Err(CoreError::Invalid("这条通知当前不能恢复到处理队列".into()));
        }
        self.store.set_notification_capture_state(
            capture_id,
            CaptureState::Queued,
            Some(if capture.local_only {
                "已恢复：仅重跑本机规则，不会发送到云端"
            } else {
                "已从回看恢复，等待下轮处理"
            }),
            None,
        )
    }

    pub fn notification_captures(&self) -> Result<Vec<NotificationCaptureRecord>> {
        self.store.list_notification_captures()
    }

    pub fn notification_filter_proposals(&self) -> Result<Vec<NotificationFilterProposal>> {
        self.store.list_notification_filter_proposals()
    }

    pub fn notification_action_proposals(&self) -> Result<Vec<NotificationActionProposal>> {
        self.store.list_notification_action_proposals()
    }

    /// Interpret one LLM hint only after local lookup finds exactly one event.
    /// The returned proposal keeps the locally-resolved id private until the
    /// user explicitly confirms it in F12; no background path can execute it.
    fn propose_notification_action(
        &self,
        capture_id: i64,
        intent: NotificationActionIntent,
        now: NaiveDateTime,
    ) -> Result<Option<NotificationActionProposal>> {
        let mut targets = self.find_event_targets(&intent.target, now)?;
        if targets.len() != 1 {
            return Ok(None);
        }
        let event = targets.pop().expect("checked exactly one target");
        let event_id = event.id.expect("store events are persisted");
        let (event_guid, _) = self.store.event_guid_and_local_only(event_id)?;
        let proposal = NotificationActionProposal {
            id: None,
            capture_id,
            kind: intent.kind,
            event_id,
            event_title: event.title,
            event_guid,
            event_start: event.start,
            new_start: intent.new_start,
            reason: intent.reason,
            state: ActionProposalState::Pending,
            created_at: now,
        };
        self.store
            .insert_notification_action_proposal(&proposal)
            .map(Some)
    }

    /// The F12 confirmation tap is the only path that may apply a
    /// Rust-resolved existing-record action proposed from notification text.
    pub fn resolve_notification_action_proposal(
        &mut self,
        proposal_id: i64,
        accepted: bool,
        now: NaiveDateTime,
    ) -> Result<String> {
        let proposal = self.store.notification_action_proposal(proposal_id)?;
        if proposal.state != ActionProposalState::Pending {
            return Err(CoreError::Invalid("该通知动作提议已处理".into()));
        }
        if !accepted {
            self.store.set_notification_action_proposal_state(
                proposal_id,
                ActionProposalState::Dismissed,
            )?;
            return Ok("已忽略通知动作提议，原日程不变。".into());
        }

        // Accepting cancels or moves a real appointment, so the card must still
        // describe the world it was written about.
        if now - proposal.created_at
            > Duration::hours(crate::notification_intelligence::ACTION_PROPOSAL_TTL_HOURS)
        {
            self.store
                .set_notification_action_proposal_state(proposal_id, ActionProposalState::Stale)?;
            return Err(CoreError::Invalid(
                "这条通知动作提议已过期，原日程未改动。如仍需处理请重新发起。".into(),
            ));
        }
        // Identity *and* state, not the row id: ids can be reused after a
        // delete, and the event may have been edited here or merged from
        // another device since the card was written. Either way the user would
        // be confirming something they were never shown.
        let current = match self.store.get_event(proposal.event_id) {
            Ok(event) => Some(event),
            Err(CoreError::NotFound(_)) => None,
            Err(e) => return Err(e),
        };
        let matches_snapshot = match &current {
            Some(event) => {
                let guid = self
                    .store
                    .event_guid_and_local_only(proposal.event_id)
                    .map(|(g, _)| g)
                    .unwrap_or_default();
                !proposal.event_guid.is_empty()
                    && guid == proposal.event_guid
                    && event.start == proposal.event_start
            }
            None => false,
        };
        if !matches_snapshot {
            self.store
                .set_notification_action_proposal_state(proposal_id, ActionProposalState::Stale)?;
            return Err(CoreError::Invalid(
                "目标日程在这条提议之后已被改动或删除，未执行任何操作。请重新确认当前日程。".into(),
            ));
        }
        let message = match proposal.kind {
            crate::notification_intelligence::NotificationActionKind::CancelEvent => {
                let event = self.cancel_event(proposal.event_id)?;
                format!("已确认取消「{}」及其未触发提醒。", event.title)
            }
            crate::notification_intelligence::NotificationActionKind::RescheduleEvent => {
                let new_start = proposal
                    .new_start
                    .ok_or_else(|| CoreError::Invalid("改期提议缺少新时间，未执行".into()))?;
                let (event, _) = self.reschedule_event(proposal.event_id, new_start, now)?;
                format!(
                    "已确认将「{}」改到 {}。",
                    event.title,
                    crate::model::fmt_ts_human(&event.start)
                )
            }
        };
        self.store
            .set_notification_action_proposal_state(proposal_id, ActionProposalState::Accepted)?;
        self.store.set_notification_capture_state(
            proposal.capture_id,
            CaptureState::Resolved,
            Some("已由你确认通知动作提议；原始通知仍保留在回看中"),
            None,
        )?;
        Ok(message)
    }

    pub fn set_notification_filter_proposal(&self, proposal_id: i64, accepted: bool) -> Result<()> {
        let proposal = self.store.set_notification_filter_proposal_state(
            proposal_id,
            if accepted {
                FilterProposalState::Accepted
            } else {
                FilterProposalState::Dismissed
            },
        )?;
        if accepted {
            let mut config = self.store.notification_intelligence_config()?;
            config.filter_rules.push(NotificationFilterRule {
                id: format!("llm-proposal:{proposal_id}"),
                pattern: proposal.pattern,
                package_name: proposal.package_name,
                matcher: proposal.matcher,
                reason: proposal.reason,
            });
            self.store.save_notification_intelligence_config(&config)?;
        }
        Ok(())
    }

    pub fn remove_notification_filter_rule(&self, rule_id: &str) -> Result<()> {
        let mut config = self.store.notification_intelligence_config()?;
        let before = config.filter_rules.len();
        config.filter_rules.retain(|rule| rule.id != rule_id);
        if config.filter_rules.len() == before {
            return Err(CoreError::NotFound(format!(
                "notification filter rule {rule_id}"
            )));
        }
        self.store.save_notification_intelligence_config(&config)
    }

    pub fn audit_log(&self) -> Result<Vec<AuditRow>> {
        self.store.list_audit()
    }

    pub fn all_notifications(&self) -> Result<Vec<Notification>> {
        self.store.list_notifications()
    }

    /// Build a self-review digest over `[from, to]` (F14 + D6 观察/记忆段).
    pub fn review(&self, from: NaiveDateTime, to: NaiveDateTime) -> Result<crate::review::Digest> {
        let raw = self.store.count_raw_inputs_between(from, to)?;
        let events = self.store.list_events()?;
        let notifs = self.store.list_notifications()?;
        let audit = self.store.list_audit()?;
        let behaviors = self.store.list_behavior()?;
        let suggestions = self.store.list_suggestions()?;
        let facts = self.store.list_facts()?;
        let soulous_facts = self.store.list_soulous_facts()?;
        Ok(crate::review::build_digest(
            from,
            to,
            raw,
            &events,
            &notifs,
            &audit,
            &behaviors,
            &suggestions,
            &facts,
            &soulous_facts,
        ))
    }

    /// The trailing-7-days review ending at `now`.
    pub fn weekly_review(&self, now: NaiveDateTime) -> Result<crate::review::Digest> {
        self.review(now - Duration::days(7), now)
    }

    /// Build today's read-only focus brief: agenda, actionable reminders, and
    /// the highest-priority pending suggestions. Storage returns full ordered
    /// record sets; [`crate::brief::build_brief`] owns the date/status windows.
    pub fn daily_brief(&self, now: NaiveDateTime) -> Result<crate::brief::Brief> {
        let events = self.store.upcoming_events(now)?;
        let notifications = self.store.list_notifications()?;
        let suggestions = self.suggestions()?;
        Ok(crate::brief::build_brief(
            now,
            &events,
            &notifications,
            &suggestions,
        ))
    }

    /// The review as user-facing text: the offline digest, rewritten in the
    /// persona's voice when a cloud reasoner is available. Returns
    /// `(text, styled)`; any cloud failure or fact-check miss degrades to the
    /// plain offline render (F16).
    pub fn review_text(&self, from: NaiveDateTime, to: NaiveDateTime) -> Result<(String, bool)> {
        let digest = self.review(from, to)?;
        if let Some(r) = &self.reasoner {
            if let Ok(Some(text)) =
                crate::llm::rewrite_digest(r.as_ref(), &digest, self.persona.as_ref())
            {
                // D6: the cloud only rephrased the numeric core; the 观察/记忆
                // extras are appended locally and never traveled upstream.
                return Ok((format!("{text}{}", digest.render_extras()), true));
            }
        }
        Ok((digest.render(), false))
    }
}
