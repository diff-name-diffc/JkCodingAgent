use super::*;

impl CapabilityBroker<'_> {
    pub(super) async fn authorize(
        &self,
        invocation: &CapabilityInvocation,
        spec: &super::super::ToolSpec,
        effective_arguments: &Value,
    ) -> Result<Value, Box<ToolResult>> {
        let resource_scope =
            self.authorize_resource_scope(invocation, spec, effective_arguments)?;
        let mut decision = match spec.safety {
            ToolSafety::Safe => json!({
                "safety": "safe",
                "decision": "allow",
            }),
            ToolSafety::Dangerous => {
                return Err(Box::new(ToolResult::fatal_error(format!(
                    "错误：工具 '{}' 被策略标记为 dangerous，运行时默认拒绝执行。",
                    invocation.name
                ))))
            }
            ToolSafety::ReviewRequired if spec.review_self_managed => {
                // 命令类工具（exec / local_zsh / ssh_exec）在内部携带完整目标环境
                // 上下文做 fail-closed 审查（目标服务器 / 执行目录、stdin、
                // 服务器级审查开关）。broker 若再做一次通用 JSON 审查，反而会用
                // 较弱的结论覆盖工具自身的审查语义，因此这里直接放行到工具内部。
                json!({
                    "safety": "review_required",
                    "decision": "allow",
                    "review": "self_managed",
                })
            }
            ToolSafety::ReviewRequired => {
                let Some(config) = self.context.ssh_review.as_ref() else {
                    return Err(Box::new(ToolResult::recoverable_error(format!(
                        "错误：工具 '{}' 需要安全审查，但当前执行上下文未配置审查模型，已按 fail-closed 拒绝。",
                        invocation.name
                    ))));
                };
                let arguments = serde_json::to_string(effective_arguments).map_err(|error| {
                    Box::new(ToolResult::recoverable_error(format!(
                        "错误：工具 '{}' 的审查参数无法序列化：{error}",
                        invocation.name
                    )))
                })?;
                let payload =
                    crate::agent::tools::review_context::build_review_payload(
                        self.context,
                        None,
                        CommandReviewTarget::AgentTool {
                            workspace_path: self.context.workspace.display().to_string(),
                            tool_name: invocation.name.clone(),
                            provider: spec.provider.clone(),
                            policy_summary: format!(
                                "readonly={}, workspaceBound={}, network={}, mutatesFilesystem={}, mutatesExternalState={}, resourceScope={}",
                                spec.access.readonly,
                                spec.access.workspace_bound,
                                spec.access.requires_network,
                                spec.access.mutates_filesystem,
                                spec.access.mutates_external_state,
                                resource_scope,
                            ),
                        },
                        arguments,
                        None,
                    );
                let verdict = review_shell_command(config, &payload)
                    .await
                    .map_err(|error| {
                        Box::new(ToolResult::recoverable_error(format!(
                            "错误：工具 '{}' 安全审查失败，已拒绝执行：{error}",
                            invocation.name
                        )))
                    })?;
                if !verdict.allowed {
                    return Err(Box::new(ToolResult::recoverable_error(
                        crate::agent::ssh_review::with_confirm_guidance(
                            format!(
                                "错误：工具 '{}' 已被安全审查拦截：{}",
                                invocation.name, verdict.reason
                            ),
                            &verdict.reason,
                        ),
                    )));
                }
                json!({
                    "safety": "review_required",
                    "decision": "allow",
                    "review": "approved",
                })
            }
        };
        if let Value::Object(policy) = &mut decision {
            policy.insert("resourceScope".to_string(), resource_scope);
        }
        Ok(decision)
    }

    pub(super) fn authorize_resource_scope(
        &self,
        invocation: &CapabilityInvocation,
        spec: &super::super::ToolSpec,
        effective_arguments: &Value,
    ) -> Result<Value, Box<ToolResult>> {
        if !self.capabilities.has_write_restriction() {
            return Ok(json!({ "mode": "unrestricted", "decision": "allow" }));
        }

        if matches!(invocation.name.as_str(), "write_file" | "edit_file") {
            let path = effective_arguments
                .get("path")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    ToolResult::recoverable_error(format!(
                        "错误：工具 '{}' 缺少可验证的 path 参数，资源授权拒绝执行。",
                        invocation.name
                    ))
                })?;
            if !self
                .capabilities
                .permits_workspace_write(&self.context.workspace, path)
            {
                return Err(Box::new(ToolResult::recoverable_error(format!(
                    "错误：工具 '{}' 请求写入 '{}'，不在当前节点 expectedFiles 授权范围内。",
                    invocation.name, path
                ))));
            }
            return Ok(json!({
                "mode": "expected_files",
                "decision": "allow",
                "path": path,
            }));
        }

        if spec.access.mutates_filesystem {
            if matches!(invocation.name.as_str(), "exec" | "local_zsh") {
                // Shell 的实际写集不能靠字符串静态推断；它必须继续经过下方
                // ReviewRequired 门禁。文件 API 则已经由 expectedFiles 精确约束。
                return Ok(json!({
                    "mode": "command_review",
                    "decision": "defer_to_safety_review",
                    "expectedFiles": self.capabilities.write_scopes_for_review(),
                }));
            }
            return Err(Box::new(ToolResult::recoverable_error(format!(
                "错误：工具 '{}' 具有文件写副作用，但当前资源授权无法绑定其目标路径，已按 fail-closed 拒绝。",
                invocation.name
            ))));
        }

        Ok(json!({ "mode": "read_or_external", "decision": "allow" }))
    }
}
