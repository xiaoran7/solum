use super::*;

impl Orchestrator {
    // ---- the closed loop --------------------------------------------------

    /// Ingest one utterance: route it, record it, and (if it's an event)
    /// extract → classify → schedule → persist.
    pub fn ingest(&mut self, text: &str, now: NaiveDateTime) -> Result<IngestOutcome> {
        self.ingest_with_stream(text, now, None, "manual", None)
    }

    /// Confirmed input from the capture candidate layer. The source identity
    /// is persisted with the raw row so later memory inspection, dedup and
    /// export do not flatten every external fact into "manual chat".
    pub fn ingest_from_source(
        &mut self,
        text: &str,
        now: NaiveDateTime,
        source_id: &str,
        external_id: Option<&str>,
    ) -> Result<IngestOutcome> {
        if let Some(external_id) = external_id {
            if let Some((raw_input_id, event)) =
                self.store.accepted_source_item(source_id, external_id)?
            {
                return Ok(IngestOutcome {
                    intent: if event.is_some() {
                        Intent::IngestEvent
                    } else {
                        Intent::Chat
                    },
                    raw_input_id,
                    event,
                    notifications: Vec::new(),
                    message: "这条来源记录已经采用过，本次已合并，没有重复创建。".into(),
                    ui: None,
                    widget_preview: None,
                });
            }
        }
        self.ingest_with_stream(text, now, None, source_id, external_id)
    }

    /// Streaming ingest (§3.6 第 7 条): identical to [`Self::ingest`] except a
    /// chat-intent reply streams its visible prose through `on_delta` as it
    /// arrives. Every other intent ignores the sink (nothing to stream).
    pub fn ingest_streaming(
        &mut self,
        text: &str,
        now: NaiveDateTime,
        on_delta: &mut dyn FnMut(&str),
    ) -> Result<IngestOutcome> {
        self.ingest_with_stream(text, now, Some(on_delta), "manual", None)
    }

    fn ingest_with_stream(
        &mut self,
        text: &str,
        now: NaiveDateTime,
        chat_sink: Option<&mut dyn FnMut(&str)>,
        source_id: &str,
        external_id: Option<&str>,
    ) -> Result<IngestOutcome> {
        let allow_ai = crate::capture::CaptureSource::parse(source_id)
            .is_none_or(crate::capture::CaptureSource::allows_ai_by_default);
        let mut intent = route_intent(text, now);
        // Explicit “组件” phrases are resolved fully offline by route_intent.
        // For a deliberately narrow ambiguous set, the user has allowed an
        // LLM-assisted classification. It only gets a yes/no vote here, never
        // a write capability; preview confirmation still bounds any mistake.
        if allow_ai && intent == Intent::Chat && might_request_widget(text, now) {
            intent = if let Some(reasoner) = &self.reasoner {
                // On a classifier failure, keep the request on the explicit
                // creation path. The schema call will then give the user a
                // clear cloud-unavailable result instead of misrouting it.
                if crate::llm::llm_routes_widget(reasoner.as_ref(), text).unwrap_or(true) {
                    Intent::CreateWidget
                } else {
                    Intent::Chat
                }
            } else {
                // An offline client cannot decide the ambiguous form safely,
                // but it must say that new-widget generation is unavailable
                // rather than silently treating it as an event or plain chat.
                Intent::CreateWidget
            };
        }
        let raw_input_id = self.store.insert_raw_input_from_source(
            text,
            intent_str(intent),
            now,
            source_id,
            external_id,
            source_id == "notification",
        )?;

        let mut outcome = IngestOutcome {
            intent,
            raw_input_id,
            event: None,
            notifications: Vec::new(),
            message: String::new(),
            ui: None,
            widget_preview: None,
        };

        match intent {
            Intent::CreateWidget => {
                if !allow_ai {
                    outcome.message =
                        "此信息源未授权用于 AI，未创建组件。你可以手动改写后再从对话发起。".into();
                    return Ok(outcome);
                }
                let Some(reasoner) = &self.reasoner else {
                    // Do not quietly turn this into a calendar event (or a
                    // half-made local widget). Existing widgets stay fully
                    // offline; only generating a new schema needs the cloud.
                    outcome.message = "创建组件需要已配置且可用的云端模型；当前没有创建任何组件。已有组件的记录仍可完全离线使用。".into();
                    return Ok(outcome);
                };
                match crate::llm::llm_widget_schema(reasoner.as_ref(), text) {
                    Ok(raw_schema) => match WidgetDefinitionDraft::parse_generated(&raw_schema) {
                        Ok(definition) => {
                            let preview_id = new_guid();
                            self.pending_widgets.insert(
                                preview_id.clone(),
                                PendingWidgetDefinition {
                                    definition: definition.clone(),
                                    raw_schema,
                                },
                            );
                            outcome.message =
                                "这是组件定义预览。请核对名称、字段和视图；确认后才会保存。".into();
                            outcome.widget_preview = Some(WidgetPreview {
                                preview_id,
                                definition,
                            });
                        }
                        Err(error) => {
                            // No partial repair: the exact rejected schema and
                            // reason become local product evidence for v2.
                            self.store.append_widget_schema_rejection(
                                &raw_schema,
                                &error.to_string(),
                                now,
                            )?;
                            outcome.message =
                                format!("生成的组件定义未通过严格校验，未创建任何组件：{error}");
                        }
                    },
                    Err(error) => {
                        outcome.message = format!(
                            "当前无法生成组件（云端不可用：{error}）。没有创建任何组件；已有组件仍可完全离线使用。"
                        );
                    }
                }
            }
            Intent::IngestEvent => {
                // LLM-first when available: let the cloud reasoner evaluate real intent,
                // pragmatics, and complex temporal offsets. If offline, unconfigured, or
                // on network failure, gracefully fall back to deterministic offline rules
                // (the reliability floor).
                let (extracted, via_llm, fallback_note) =
                    if let (true, Some(r)) = (allow_ai, &self.reasoner) {
                        match crate::llm::llm_extract(r.as_ref(), text, now) {
                            Ok(llm_ev) => {
                                let is_llm = llm_ev.is_some();
                                (llm_ev, is_llm, None)
                            }
                            Err(_error) => {
                                // Network/gateway error: trigger offline rule fallback as safety net
                                let fallback_ev = self.extractor.extract(text, now)?;
                                let note = fallback_ev
                                    .is_some()
                                    .then_some("（云端暂不可用，已通过本地规则为你创建）");
                                (fallback_ev, false, note)
                            }
                        }
                    } else {
                        // Offline / guest / unconfigured / local-only: deterministic rules floor
                        (self.extractor.extract(text, now)?, false, None)
                    };

                if let Some(ev) = extracted {
                    if is_daily_recurrence(text) {
                        let routine = self.persist_daily_routine(&ev, raw_input_id, now)?;
                        outcome.message = if routine.is_some() {
                            format!(
                                "已创建每天 {} 的固定提醒「{}」。将通过常规提醒链路触发，可在台账暂停或删除。",
                                ev.start.format("%H:%M"),
                                ev.title
                            )
                        } else {
                            format!(
                                "每天 {} 的固定提醒「{}」已经启用，未重复创建。",
                                ev.start.format("%H:%M"),
                                ev.title
                            )
                        };
                        if via_llm {
                            outcome.message.push_str("（由云端解析）");
                        } else if let Some(note) = fallback_note {
                            outcome.message.push_str(note);
                        }
                        if let Some(routine) = routine {
                            outcome.ui = crate::genui::routine_ingested(&routine);
                        }
                    } else {
                        let (ev, stored) = self.persist_event(ev, raw_input_id)?;
                        outcome.message = summarize_event(&ev, &stored, now);
                        if via_llm {
                            outcome.message.push_str("（由云端解析）");
                        } else if let Some(note) = fallback_note {
                            outcome.message.push_str(note);
                        }
                        if let Some(caveat) = recurrence_caveat(text) {
                            outcome.message.push_str(&format!("\n{caveat}"));
                        }
                        // F18: confirmation card + reminder actions, offline template.
                        outcome.ui = Some(crate::genui::event_ingested(&ev, &stored));
                        outcome.event = Some(ev);
                        outcome.notifications = stored;
                    }
                } else {
                    outcome.message = "识别为事件，但没能抽取出时间，已作为普通输入记录。".into();
                }
            }
            Intent::DangerousCommand => {
                // D3: a purge-shaped request gets a guard *entrance* — one
                // danger button that starts the §3.3 flow (preview with the
                // real row count → human confirm → one-time token). Anything
                // else keeps the generic hard-stop message.
                if let Some((layer, before)) = parse_purge_request(text, now) {
                    let tool_args = serde_json::json!({
                        "layer": layer.as_str(),
                        "before": fmt_ts(&before),
                    })
                    .to_string();
                    let count = self.store.count_memory_before(layer, before)?;
                    outcome.message = format!(
                        "这是不可恢复的批量删除（{}层，早于 {}，当前匹配 {count} 条）。\
                         需要你在确认弹窗里核对后才会执行。",
                        layer_label(layer),
                        crate::model::fmt_ts_human(&before)
                    );
                    outcome.ui = Some(crate::genui::guard_entry(
                        &outcome.message,
                        "发起删除确认",
                        "ledger_purge",
                        &tool_args,
                    ));
                } else {
                    outcome.message =
                        "⚠️ 检测到高危操作。任何主动模式下都不会自动执行，需要你显式确认后才能进行。".into();
                }
            }
            Intent::RescheduleEvent => {
                let req = crate::extract::parse_reschedule(text, now)
                    .ok_or_else(|| CoreError::Invalid("RescheduleEvent 却解析不出请求".into()))?;
                let targets = self.find_event_targets(&req.target, now)?;
                match targets.len() {
                    0 => {
                        outcome.message =
                            "没找到要改期的日程（只在未来的日程里找）。可以说得更具体些，比如带上标题或日期。"
                                .into();
                    }
                    1 => {
                        let ev = &targets[0];
                        let new_start = compose_new_start(ev, &req);
                        let (ev, stored) =
                            self.reschedule_event(ev.id.expect("persisted"), new_start, now)?;
                        outcome.message = format!(
                            "已把「{}」改到 {}。{}",
                            ev.title,
                            crate::model::fmt_ts_human(&ev.start),
                            summarize_reminders(&stored, now)
                        );
                        outcome.ui = Some(crate::genui::event_ingested(&ev, &stored));
                        outcome.event = Some(ev);
                        outcome.notifications = stored;
                    }
                    _ => {
                        outcome.message = format!(
                            "找到 {} 个匹配的日程，点一个确认改期（或说得更具体些）。",
                            targets.len()
                        );
                        let cands: Vec<(Event, NaiveDateTime)> = targets
                            .iter()
                            .map(|ev| (ev.clone(), compose_new_start(ev, &req)))
                            .collect();
                        outcome.ui = crate::genui::reschedule_pick(&outcome.message, &cands);
                    }
                }
            }
            Intent::CancelEvent => {
                let target = crate::extract::parse_cancel(text)
                    .ok_or_else(|| CoreError::Invalid("CancelEvent 却解析不出目标".into()))?;
                let targets = self.find_event_targets(&target, now)?;
                if targets.is_empty() {
                    outcome.message =
                        "没找到要取消的日程（只在未来的日程里找）。可以说得更具体些，或到日程视图看看。"
                            .into();
                } else {
                    // Deletion is destructive: never straight off an utterance.
                    // The tap on the named (danger) button is the confirmation.
                    outcome.message = if targets.len() == 1 {
                        format!(
                            "确认取消「{}」（{}）？取消会连带删除它派生的提醒。",
                            targets[0].title,
                            crate::model::fmt_ts_human(&targets[0].start)
                        )
                    } else {
                        format!(
                            "找到 {} 个匹配的日程，点一个确认取消（会连带删除其提醒）。",
                            targets.len()
                        )
                    };
                    outcome.ui = crate::genui::cancel_confirm(&outcome.message, &targets);
                }
            }
            Intent::MemoryWrite => {
                // §3.10 M2: rule-matched "记住…" writes a semantic fact.
                let content = crate::memory::extract_fact_content(text)
                    .ok_or_else(|| CoreError::Invalid("MemoryWrite 却抽不出内容".into()))?;
                let fact = MemoryFact {
                    id: None,
                    content: content.clone(),
                    source: "chat".into(),
                    created_at: now,
                    last_used_at: None,
                };
                outcome.message = match self.store.insert_fact_if_new(&fact)? {
                    Some(_) => format!("已记住：{content}（可在记忆台账查看或删除）"),
                    None => format!("这条我已经记着了：{content}"),
                };
            }
            Intent::StatusAnswer => {
                // F4: status answers condense into the behavior journal.
                let activity = strip_status_prefix(text);
                self.store.insert_behavior(&BehaviorEntry {
                    id: None,
                    ts: now,
                    kind: BehaviorKind::Status,
                    content: activity.clone(),
                    source: Some(format!("raw_input#{raw_input_id}")),
                })?;
                outcome.message = format!("已记录你的当前状态：{activity}。");
            }
            Intent::Chat => {
                if !allow_ai {
                    outcome.message =
                        "这条来源内容未发送给 AI，也没有形成日程。你可以补充明确时间后重试。"
                            .into();
                    return Ok(outcome);
                }
                match &self.reasoner {
                    Some(r) => {
                        // §3.10: retrieve locally *before* the call — snippets
                        // + recent turns are the only context that travels.
                        let snippets = self.recall(text, now)?;
                        let history: Vec<ChatTurn> = self.chat_history.iter().cloned().collect();
                        let ctx = ChatContext {
                            history: &history,
                            snippets: &snippets,
                        };
                        // F18: one call, envelope-or-prose; any failure inside
                        // chat_reply_ui already degraded to plain text. With a
                        // sink, plain-prose replies stream token by token
                        // (§3.6 第 7 条); the envelope path is identical either way.
                        let reply = match chat_sink {
                            Some(sink) => crate::llm::chat_reply_ui_streaming(
                                r.as_ref(),
                                text,
                                now,
                                self.persona.as_ref(),
                                &ctx,
                                sink,
                            ),
                            None => crate::llm::chat_reply_ui(
                                r.as_ref(),
                                text,
                                now,
                                self.persona.as_ref(),
                                &ctx,
                            ),
                        };
                        match reply {
                            Ok((msg, ui)) => {
                                outcome.message = msg;
                                outcome.ui = ui;
                            }
                            Err(e) => outcome.message = format!("已记录。（云端暂不可用：{e}）"),
                        }
                        // M1: remember this exchange for the next few calls.
                        self.chat_history.push_back(ChatTurn {
                            user: text.to_string(),
                            assistant: outcome.message.clone(),
                        });
                        while self.chat_history.len() > MAX_HISTORY_TURNS {
                            self.chat_history.pop_front();
                        }
                    }
                    None => {
                        return Err(CoreError::Llm("使用云端智能能力请先登录 Solum 账号".into()));
                    }
                };
            }
        }
        Ok(outcome)
    }

    /// F1/F2: ingest text captured from device notifications (the Android
    /// listener). Unlike [`Self::ingest`] this **never touches the cloud
    /// reasoner** and silently drops anything the offline rules can't turn
    /// into an event. Phase 9 only decides whether a successfully captured
    /// row is `local_only`; it does not add LLM triage or any new action path.
    /// Dropped text is not stored anywhere either: no hoarding.
    pub fn ingest_captured(
        &mut self,
        text: &str,
        origin: &str,
        now: NaiveDateTime,
    ) -> Result<Option<IngestOutcome>> {
        if route_intent(text, now) != Intent::IngestEvent {
            return Ok(None);
        }
        let Some(ev) = self.extractor.extract(text, now)? else {
            return Ok(None);
        };
        let raw = format!("[{origin}] {text}");
        let local_only = !self.store.notif_cloud_enabled()?;
        let raw_input_id = if local_only {
            self.store.insert_raw_input_from_source(
                &raw,
                intent_str(Intent::IngestEvent),
                now,
                "notification",
                None,
                true,
            )?
        } else {
            self.store.insert_raw_input_from_source(
                &raw,
                intent_str(Intent::IngestEvent),
                now,
                "notification",
                None,
                false,
            )?
        };
        let (ev, stored) = self.persist_event_with_scope(ev, raw_input_id, local_only)?;
        Ok(Some(IngestOutcome {
            intent: Intent::IngestEvent,
            raw_input_id,
            message: summarize_event(&ev, &stored, now),
            // Captured events surface as OS notifications, not chat bubbles —
            // no envelope to render.
            ui: None,
            widget_preview: None,
            event: Some(ev),
            notifications: stored,
        }))
    }

    /// Classify → schedule → persist one extracted event.
    pub(crate) fn persist_event(
        &mut self,
        ev: Event,
        raw_input_id: i64,
    ) -> Result<(Event, Vec<Notification>)> {
        self.persist_event_with_scope(ev, raw_input_id, false)
    }

    /// Convert an explicit daily schedule into the existing `routines` model.
    /// It is deliberately limited to a daily cadence: weekly/monthly phrases
    /// require a different recurrence model and must not be faked as a daily
    /// or one-shot event.
    fn persist_daily_routine(
        &mut self,
        ev: &Event,
        raw_input_id: i64,
        now: NaiveDateTime,
    ) -> Result<Option<Routine>> {
        let mut routine = Routine {
            id: None,
            title: ev.title.clone(),
            time_of_day: ev.start.format("%H:%M").to_string(),
            source: Some(format!("raw_input#{raw_input_id}")),
            active: true,
            created_at: ev.created_at,
            scheduled_until: None,
        };
        let Some(id) = self.store.insert_routine_if_new(&routine)? else {
            return Ok(None);
        };
        routine.id = Some(id);
        // Make the just-created routine visible to the normal reminder,
        // alarm-mirror, sync, and ledger paths without waiting for the
        // next resident ticker pass.
        self.materialize_routines(now)?;
        Ok(Some(routine))
    }

    pub(crate) fn persist_event_with_scope(
        &mut self,
        mut ev: Event,
        raw_input_id: i64,
        local_only: bool,
    ) -> Result<(Event, Vec<Notification>)> {
        // One unit: an event whose reminder insert fails halfway must not
        // survive with a partial plan (insert_notification failing used to
        // leave exactly that).
        let (ev, stored) = self.store.with_transaction(|s| {
            let ev_id = s.insert_event_with_scope(&ev, Some(raw_input_id), local_only, None)?;
            ev.id = Some(ev_id);
            let rule = self.rule_table.rule(ev.kind);
            let planned = plan_notifications(&ev, &rule, ev.created_at, ev_id);
            let mut stored = Vec::with_capacity(planned.len());
            for mut n in planned {
                let nid = s.insert_notification(&n)?;
                n.id = Some(nid);
                stored.push(n);
            }
            Ok((ev, stored))
        })?;
        Ok((ev, stored))
    }


}
