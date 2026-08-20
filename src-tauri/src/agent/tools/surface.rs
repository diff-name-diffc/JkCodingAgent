use crate::agent::llm::ToolDefinition;

use super::CapabilitySet;

/// 一轮 LLM 调用的工具暴露面。
///
/// `definitions`/`direct_capabilities` 描述模型能直接调用什么；
/// `runtime_capabilities` 描述运行时工具可代理什么。二者刻意分离，避免再从
/// 可见 definitions 反推宿主权限。
#[derive(Clone, Debug)]
pub struct ToolSurface {
    pub definitions: Vec<ToolDefinition>,
    pub direct_capabilities: CapabilitySet,
    pub runtime_capabilities: CapabilitySet,
}

impl ToolSurface {
    /// 普通模式：模型可见工具与直接可调用能力完全一致，没有额外运行时授权。
    pub fn direct(definitions: Vec<ToolDefinition>) -> Self {
        let direct_capabilities = CapabilitySet::from_definitions(&definitions);
        Self {
            definitions,
            direct_capabilities,
            runtime_capabilities: CapabilitySet::default(),
        }
    }

    /// 分层模式：模型仅看到外层工具；真实数据面能力只授权给受限运行时。
    pub fn layered(definitions: Vec<ToolDefinition>, runtime_capabilities: CapabilitySet) -> Self {
        let direct_capabilities = CapabilitySet::from_definitions(&definitions);
        Self {
            definitions,
            direct_capabilities,
            runtime_capabilities,
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::ToolSurface;
    use crate::agent::llm::{ToolDefinition, ToolFunctionDefinition};
    use crate::agent::tools::CapabilitySet;

    fn definition(name: &str) -> ToolDefinition {
        ToolDefinition {
            kind: "function".to_string(),
            function: ToolFunctionDefinition {
                name: name.to_string(),
                description: String::new(),
                parameters: json!({ "type": "object" }),
            },
        }
    }

    #[test]
    fn layered_surface_does_not_leak_runtime_capabilities_to_the_model() {
        let surface = ToolSurface::layered(
            vec![definition("run_tool_program"), definition("message")],
            CapabilitySet::new(["read_file".to_string(), "grep".to_string()]),
        );

        assert!(surface.direct_capabilities.contains("run_tool_program"));
        assert!(!surface.direct_capabilities.contains("read_file"));
        assert!(surface.runtime_capabilities.contains("read_file"));
        assert_eq!(surface.definitions.len(), 2);
    }
}
