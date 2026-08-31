use super::*;

impl SubAgentRuntime {
    /// 构建子智能体运行时。provider 可继承父级配置或使用子智能体独立配置；
    /// 关键约束：排除嵌套的 call_sub_agent / list_sub_agents 工具，防止子智能体
    /// 递归派生导致无限调用栈。
    pub fn build(
        config: &SubAgentConfig,
        parent_provider: &OpenAiCompatProvider,
        tool_registry: Arc<ToolRegistry>,
        tool_context: ToolContext,
    ) -> Result<Self> {
        let provider = if config.model_config.inherit_from_parent {
            let model_name = config
                .model_config
                .model_name
                .as_deref()
                .filter(|s| !s.is_empty())
                .unwrap_or(parent_provider.model());
            parent_provider.with_model(model_name)
        } else {
            let api_base = config
                .model_config
                .api_base
                .as_deref()
                .unwrap_or(parent_provider.api_base());
            let api_key = config
                .model_config
                .api_key
                .as_deref()
                .unwrap_or(parent_provider.api_key());
            let model_name = config
                .model_config
                .model_name
                .as_deref()
                .unwrap_or(parent_provider.model());
            OpenAiCompatProvider::new(
                api_key.to_string(),
                api_base.to_string(),
                model_name.to_string(),
                config.max_output_tokens,
                config.temperature as f32,
            )
        };

        let mut tool_context = tool_context;
        let parent_tool_call_id = tool_context
            .current_tool_call_id
            .clone()
            .ok_or_else(|| anyhow::anyhow!("构建子智能体运行时缺少父级 tool_call_id"))?;
        // G1-20：缓冲容量治理在写入端（record_trace_event）强制执行；
        // 这里按上限预分配，避免运行期反复扩容。
        let trace_events = Arc::new(Mutex::new(Vec::with_capacity(SUB_AGENT_TRACE_EVENT_LIMIT)));
        tool_context.current_sub_agent_id = Some(config.agent_id.clone());
        tool_context.current_sub_agent_name = Some(config.agent_name.clone());
        tool_context.sub_agent_parent_tool_call_id = Some(parent_tool_call_id.clone());
        tool_context.sub_agent_trace_events = Some(Arc::clone(&trace_events));

        // 排除嵌套子智能体工具（call_sub_agent / list_sub_agents），避免递归派生。
        let excluded: HashSet<&str> = NESTED_SUB_AGENT_TOOLS.iter().copied().collect();
        let mut nested_tools = config
            .allowed_tools
            .iter()
            .filter(|name| excluded.contains(name.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        nested_tools.sort();
        if !nested_tools.is_empty() {
            anyhow::bail!(
                "错误：子智能体 '{}' 不允许递归调用子智能体工具：{}。请在设置中移除这些工具。",
                config.agent_name,
                nested_tools.join("、")
            );
        }
        let allowed_tool_names: HashSet<String> = config.allowed_tools.iter().cloned().collect();

        let tool_definitions = tool_registry.definitions_for_scope(
            &tool_context.mcp_scope,
            Some(allowed_tool_names.iter().map(String::as_str)),
            false,
        );
        let resolved_tool_names = tool_definitions
            .iter()
            .map(|definition| definition.function.name.clone())
            .collect::<HashSet<_>>();
        let mut unavailable = allowed_tool_names
            .difference(&resolved_tool_names)
            .cloned()
            .collect::<Vec<_>>();
        unavailable.sort();
        if !unavailable.is_empty() {
            anyhow::bail!(
                "错误：子智能体 '{}' 配置了当前普通聊天执行环境不可用的工具：{}。请在设置中重新选择工具。",
                config.agent_name,
                unavailable.join("、")
            );
        }
        let capabilities = CapabilitySet::from_definitions(&tool_definitions);

        Ok(Self {
            config: config.clone(),
            provider,
            tool_registry,
            tool_context,
            tool_definitions,
            capabilities,
            parent_tool_call_id,
            trace_events,
        })
    }

    pub fn trace_events_json(&self) -> Result<String> {
        // G13-08：只在持锁期间 clone 出事件列表，序列化放到锁外执行，
        // 避免长任务下对数千条事件做 serde_json::to_string 时长时间持锁，
        // 阻塞 emit_event / record_trace_event 等所有写入路径。
        let events = self.trace_events.lock().clone();
        serde_json::to_string(&events)
            .map_err(|error| anyhow::anyhow!("serialize sub-agent trace events: {error}"))
    }

    pub(super) fn emit_event(
        &self,
        app_handle: &Option<AppHandle>,
        session_id: &str,
        event: SubAgentEvent,
    ) {
        let timestamp_ms = chrono::Utc::now().timestamp_millis();
        if let Ok(value) = serde_json::to_value(&event) {
            record_trace_event(&self.trace_events, value, timestamp_ms);
        }
        if let Some(handle) = app_handle {
            let _ = handle.emit(
                "sub-agent-event",
                SubAgentEventPayload {
                    session_id: session_id.to_string(),
                    tool_call_id: self.parent_tool_call_id.clone(),
                    timestamp_ms,
                    event,
                },
            );
        }
    }
}
