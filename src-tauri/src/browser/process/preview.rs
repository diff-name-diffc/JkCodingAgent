use super::*;

impl BrowserProcess {
    pub(in crate::browser) fn claim_preview_worker(&self) -> bool {
        !self.preview_attached.swap(true, Ordering::AcqRel)
    }

    pub(in crate::browser) fn termination_receiver(&self) -> watch::Receiver<bool> {
        self.termination_done.subscribe()
    }

    pub(in crate::browser) fn report_preview_error(&self, message: String) {
        let tool_call_id = self
            .agent
            .lock()
            .as_ref()
            .map(|binding| binding.tool_call_id.clone());
        let payload = json!({"sessionId":self.session_id,"streamId":self.stream_id,
            "toolCallId":tool_call_id,"message":message});
        (self.events)("browser-preview-error", payload.clone());
        (self.events)("browser-log", payload);
    }

    fn matches_preview_context(&self, value: &Value) -> bool {
        let binding = self.agent.lock();
        match binding.as_ref() {
            Some(binding) => {
                value.get("runId").and_then(Value::as_str) == Some(binding.run_id.as_str())
                    && value.get("toolCallId").and_then(Value::as_str)
                        == Some(binding.tool_call_id.as_str())
            }
            None => {
                value.get("runId").is_some_and(Value::is_null)
                    && value.get("toolCallId").is_some_and(Value::is_null)
            }
        }
    }

    pub(super) fn handle_preview_error(&self, mut value: Value) {
        if !self.matches_preview_context(&value) {
            return;
        }
        value
            .as_object_mut()
            .expect("已校验预览事件对象")
            .insert("streamId".into(), json!(self.stream_id));
        (self.events)("browser-preview-error", value.clone());
        (self.events)("browser-log", value);
    }

    pub(super) fn handle_frame(&self, mut value: Value) {
        if !self.matches_preview_context(&value)
            || value.get("capturedAt").and_then(Value::as_u64).is_none()
        {
            return;
        }
        let Some(sequence) = value.get("sequence").and_then(Value::as_u64) else {
            return;
        };
        if self
            .last_frame_sequence
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |previous| {
                (sequence > previous).then_some(sequence)
            })
            .is_err()
        {
            return;
        }
        value
            .as_object_mut()
            .expect("已校验帧对象")
            .insert("streamId".into(), json!(self.stream_id));
        (self.events)("browser-frame", value);
    }
}
