use super::*;

impl Orchestrator {
    // ---- persona (F9 v1 / F15) ----------------------------------------------

    /// The active persona, if the user has set one.
    pub fn persona(&self) -> Option<&PersonaProfile> {
        self.persona.as_ref()
    }

    /// All persona versions, newest first (the F15 history).
    pub fn persona_versions(&self) -> Result<Vec<PersonaProfile>> {
        self.store.list_persona_versions()
    }

    /// Save manual style settings as a new persona version and make it active.
    pub fn set_persona(
        &mut self,
        draft: PersonaDraft,
        note: Option<String>,
        now: NaiveDateTime,
    ) -> Result<PersonaProfile> {
        let draft = draft.normalized()?;
        let profile = self
            .store
            .insert_persona_version(&draft, "manual", note, now)?;
        self.persona = Some(profile.clone());
        Ok(profile)
    }

    /// Run the local chat-log extraction (F9, §3.4) and return the report +
    /// suggested draft. Pure preview: nothing is stored, nothing leaves the
    /// device — the user reviews/edits the draft before saving.
    pub fn preview_persona_import(
        &self,
        raw: &str,
        me: &str,
    ) -> Result<crate::persona_import::ImportReport> {
        crate::persona_import::extract_persona(raw, me)
    }

    /// Save a user-confirmed import draft as a new persona version
    /// (`source = "import"`) and make it active. The draft may have been
    /// edited after [`Self::preview_persona_import`]; the raw chat log is
    /// never stored.
    pub fn import_persona(
        &mut self,
        draft: PersonaDraft,
        note: Option<String>,
        now: NaiveDateTime,
    ) -> Result<PersonaProfile> {
        let draft = draft.normalized()?;
        let profile = self
            .store
            .insert_persona_version(&draft, "import", note, now)?;
        self.persona = Some(profile.clone());
        Ok(profile)
    }

    /// Point the active persona back at an earlier version (history is kept).
    pub fn rollback_persona(&mut self, version: i64) -> Result<PersonaProfile> {
        let profile = self.store.set_active_persona(version)?;
        self.persona = Some(profile.clone());
        Ok(profile)
    }

    /// Delete every persona version (right-to-delete; F12 spirit).
    pub fn clear_persona(&mut self) -> Result<()> {
        self.store.clear_persona()?;
        self.persona = None;
        self.audit_irreversible("persona_clear", "删除全部人格版本与活动指针");
        Ok(())
    }

    // ---- persistent widgets (F19) ---------------------------------------

    /// Step 2 of widget creation: consume a previously rendered preview and
    /// persist it. There is intentionally no direct “create arbitrary schema”
    /// public path in the shell; confirmation is the safety boundary.
    pub fn confirm_widget_preview(
        &mut self,
        preview_id: &str,
        now: NaiveDateTime,
    ) -> Result<WidgetDefinition> {
        let pending = self
            .pending_widgets
            .remove(preview_id)
            .ok_or_else(|| CoreError::NotFound(format!("widget_preview#{preview_id}")))?;
        match self
            .store
            .insert_widget_definition(&pending.definition, now)
        {
            Ok(definition) => Ok(definition),
            Err(error) => {
                self.store.append_widget_schema_rejection(
                    &pending.raw_schema,
                    &error.to_string(),
                    now,
                )?;
                Err(error)
            }
        }
    }

    /// Dismissing a preview is a no-op for durable data. It merely releases
    /// the in-process pending definition so it cannot later be confirmed.
    pub fn discard_widget_preview(&mut self, preview_id: &str) -> Result<()> {
        self.pending_widgets
            .remove(preview_id)
            .map(|_| ())
            .ok_or_else(|| CoreError::NotFound(format!("widget_preview#{preview_id}")))
    }

    /// Append an optional field to an existing widget (设计稿 ⑧). `safe`: it
    /// cannot lose data, which is precisely why it is the only evolution
    /// operation offered — removing a field or changing its type would.
    pub fn add_widget_field(
        &self,
        widget_id: i64,
        field: &crate::widget::WidgetField,
        now: NaiveDateTime,
    ) -> Result<WidgetDefinition> {
        self.store.add_widget_field(widget_id, field, now)
    }

    /// 设计稿 ⑦-A: seed a fresh widget from existing schedule entries, so it
    /// does not open onto an empty form. A **snapshot**: later edits to the
    /// event do not reach the copy, and the UI has to say so.
    pub fn import_events_into_widget(
        &self,
        widget_id: i64,
        limit: usize,
        now: NaiveDateTime,
    ) -> Result<WidgetImportOutcome> {
        let definition = self.store.get_widget_definition(widget_id)?;
        if definition.schema.event_mapping().is_none() {
            return Err(CoreError::Invalid(
                "该组件没有文本字段，无法承载日程标题".into(),
            ));
        }
        let mut outcome = WidgetImportOutcome {
            imported: 0,
            skipped: 0,
            reasons: Vec::new(),
        };
        let note = |outcome: &mut WidgetImportOutcome, title: &str, reason: String| {
            outcome.skipped += 1;
            if outcome.reasons.len() < crate::widget::MAX_SKIP_REASONS {
                outcome.reasons.push(crate::widget::WidgetImportSkip {
                    title: title.to_string(),
                    reason,
                });
            }
        };
        for event in self.store.list_events()? {
            // `limit` caps how many records get *written*, not how far down the
            // schedule we may look. Applying it to the scan meant a run of
            // unmappable entries at the top could hide every importable one
            // behind them, and the whole run reported a bare 0.
            if outcome.imported >= limit {
                break;
            }
            let Some(data) = definition
                .schema
                .record_from_event(&event.title, event.start)
            else {
                note(
                    &mut outcome,
                    &event.title,
                    "组件没有可承载标题的文本字段".into(),
                );
                continue;
            };
            // A mapping that cannot satisfy the schema's required fields is
            // skipped, not patched with invented values — but the user is told
            // which entry and why, because "导入 0 条" on its own is unactionable.
            if let Err(error) = definition.schema.validate_record(&data) {
                // The reason is shown to the user verbatim, so unwrap the
                // variant's own message instead of letting the `invalid input:`
                // Display prefix leak into a toast.
                let reason = match &error {
                    CoreError::Invalid(message) => message.clone(),
                    other => other.to_string(),
                };
                note(&mut outcome, &event.title, reason);
                continue;
            }
            self.store.insert_widget_record(widget_id, &data, now)?;
            outcome.imported += 1;
        }
        Ok(outcome)
    }

    /// 设计稿 ⑦-C: promote a widget record into a real schedule entry. Also a
    /// snapshot — the new event does not stay tied to the record.
    pub fn promote_widget_record(
        &mut self,
        widget_id: i64,
        record_id: i64,
        now: NaiveDateTime,
    ) -> Result<Event> {
        let definition = self.store.get_widget_definition(widget_id)?;
        let record = self
            .store
            .list_widget_records(widget_id)?
            .into_iter()
            .find(|r| r.id == record_id)
            .ok_or_else(|| CoreError::NotFound(format!("widget_record#{record_id}")))?;
        let (title, when) = definition
            .schema
            .event_from_record(&record.data)
            .ok_or_else(|| CoreError::Invalid("该记录没有可用作日程标题的文本内容".into()))?;
        let start = when.ok_or_else(|| {
            CoreError::Invalid("该记录没有日期或时间字段，无法确定日程时间".into())
        })?;
        // Provenance first: F12 shows every event under the input it came
        // from, so a promoted record needs a raw input of its own rather than
        // appearing in the ledger from nowhere.
        let provenance = format!("[组件·{}] {}", definition.name, title);
        let raw_input_id =
            self.store
                .insert_raw_input(&provenance, "promote_widget_record", now)?;
        let event = Event::new(&title, EventKind::Other, start, &provenance, now);
        // Goes through the ordinary path, so reminders are planned by the same
        // rule table as any other event — a promoted row is not a second-class
        // event with its own scheduling logic.
        let (event, _) = self.persist_event(event, raw_input_id)?;
        Ok(event)
    }

    pub fn widget_definitions(&self) -> Result<Vec<WidgetDefinition>> {
        self.store.list_widget_definitions()
    }

    pub fn widget_records(&self, widget_id: i64) -> Result<Vec<WidgetRecord>> {
        self.store.list_widget_records(widget_id)
    }

    pub fn add_widget_record(
        &self,
        widget_id: i64,
        data: serde_json::Value,
        now: NaiveDateTime,
    ) -> Result<WidgetRecord> {
        self.store.insert_widget_record(widget_id, &data, now)
    }

    pub fn update_widget_record(
        &self,
        widget_id: i64,
        record_id: i64,
        data: serde_json::Value,
    ) -> Result<WidgetRecord> {
        self.store.update_widget_record(widget_id, record_id, &data)
    }

    pub fn delete_widget_record(&self, widget_id: i64, record_id: i64) -> Result<()> {
        self.store.delete_widget_record(widget_id, record_id)?;
        self.audit_irreversible(
            "widget_record_delete",
            &format!("删除组件 #{widget_id} 的记录 #{record_id}"),
        );
        Ok(())
    }


}
