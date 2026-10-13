use super::*;

impl SubAgentDb {
    /// 仅在应用启动、所有执行器尚未启动时调用。崩溃前的 checkpoint 不可假装仍在执行。
    pub fn recover_interrupted_runs(&self) -> Result<()> {
        let mut conn = self.conn()?;
        let tx = conn
            .transaction()
            .context("begin interrupted sub-agent recovery")?;
        let rows = {
            let mut stmt = tx.prepare("SELECT workspace_id, tool_call_id, agent_id, events_json FROM sub_agent_run_traces WHERE status = 'running'")?;
            let rows = stmt
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows
        };
        for (workspace, tool_call, agent_id, raw) in rows {
            // 崩溃可能留下半写的 events_json；单行损坏不能阻断整个应用启动，
            // 该轨迹丢弃原始事件、以合成的中断事件收口，其余轨迹继续恢复。
            let mut events: Vec<serde_json::Value> = match serde_json::from_str(&raw) {
                Ok(events) => events,
                Err(error) => {
                    eprintln!(
                        "中断的子智能体轨迹无法解析，已用合成事件收口（{workspace}/{tool_call}）：{error}"
                    );
                    Vec::new()
                }
            };
            let message = "应用在浏览器探索期间退出；执行已中断，请根据当前页面重新发起任务";
            let timestamp = Utc::now().timestamp_millis();
            let mut active = std::collections::BTreeMap::new();
            for event in &events {
                let Some(id) = event["data"]["taskId"].as_str() else {
                    continue;
                };
                match event["event"].as_str() {
                    Some("ToolStarted") => {
                        active.insert(id.to_owned(), event["data"].clone());
                    }
                    Some("ToolFinished") => {
                        active.remove(id);
                    }
                    _ => {}
                }
            }
            for (_, mut data) in active {
                data["resultPreview"] = message.into();
                data["error"] = message.into();
                if let Some(object) = data.as_object_mut() {
                    object.remove("arguments");
                }
                events.push(
                    serde_json::json!({"event":"ToolFinished","timestampMs":timestamp,"data":data}),
                );
            }
            let agent_name = events
                .iter()
                .find_map(|event| event["data"]["agentName"].as_str())
                .map(str::to_owned)
                .unwrap_or_else(|| agent_id.clone());
            events.push(serde_json::json!({"event":"Failed","timestampMs":timestamp,
                "data":{"agentId":agent_id,"agentName":agent_name,"error":message}}));
            tx.execute("UPDATE sub_agent_run_traces SET status='failed', events_json=?1, updated_at=?2 WHERE workspace_id=?3 AND tool_call_id=?4 AND status='running'",
                params![serde_json::to_string(&events)?, Utc::now().to_rfc3339(), workspace, tool_call])?;
        }
        tx.commit().context("commit interrupted sub-agent recovery")
    }

    pub fn save_run_trace(
        &self,
        workspace_id: &str,
        tool_call_id: &str,
        agent_id: &str,
        status: &str,
        events_json: &str,
        model: Option<&str>,
    ) -> Result<SubAgentRunTraceRecord> {
        if workspace_id.trim().is_empty() || tool_call_id.trim().is_empty() {
            anyhow::bail!("workspace_id 和 tool_call_id 不能为空");
        }
        serde_json::from_str::<serde_json::Value>(events_json)
            .context("validate sub-agent trace events_json")?;
        let conn = self.conn()?;
        let timestamp = Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO sub_agent_run_traces (
                workspace_id, tool_call_id, agent_id, status, events_json, model, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)
             ON CONFLICT(workspace_id, tool_call_id) DO UPDATE SET
                agent_id = excluded.agent_id,
                status = excluded.status,
                events_json = excluded.events_json,
                model = excluded.model,
                updated_at = excluded.updated_at",
            params![
                workspace_id,
                tool_call_id,
                agent_id,
                status,
                events_json,
                model,
                timestamp
            ],
        )
        .context("save sub-agent run trace")?;
        self.get_run_trace(workspace_id, tool_call_id)?
            .context("sub-agent run trace missing after save")
    }

    pub fn get_run_trace(
        &self,
        workspace_id: &str,
        tool_call_id: &str,
    ) -> Result<Option<SubAgentRunTraceRecord>> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT workspace_id, tool_call_id, agent_id, status, events_json, model, created_at, updated_at
             FROM sub_agent_run_traces
             WHERE workspace_id = ?1 AND tool_call_id = ?2",
            params![workspace_id, tool_call_id],
            |row| {
                Ok(SubAgentRunTraceRecord {
                    workspace_id: row.get(0)?,
                    tool_call_id: row.get(1)?,
                    agent_id: row.get(2)?,
                    status: row.get(3)?,
                    events_json: row.get(4)?,
                    model: row.get(5)?,
                    created_at: row.get(6)?,
                    updated_at: row.get(7)?,
                })
            },
        )
        .optional()
        .context("get sub-agent run trace")
    }
}
