# Sub-Agent（子智能体）概要设计文档

> **版本**: v1.0  
> **日期**: 2026-06-05  
> **状态**: 草案

---

## 目录

1. [背景与动机](#1-背景与动机)
2. [核心设计理念](#2-核心设计理念)
3. [架构总览](#3-架构总览)
4. [数据模型设计](#4-数据模型设计)
5. [后端：Sub-Agent Runtime 框架](#5-后端sub-agent-runtime-框架)
6. [接口设计](#6-接口设计)
7. [前端：管理界面设计](#7-前端管理界面设计)
8. [浏览器子智能体专项设计](#8-浏览器子智能体专项设计)
9. [迁移与兼容方案](#9-迁移与兼容方案)
10. [实施路线](#10-实施路线)

---

## 1. 背景与动机

### 1.1 问题陈述

当前 Aha 智能体（Dispatcher Agent）以**扁平工具调用**的方式直接使用 8 个浏览器工具（`browser_open_url`、`browser_click`、`browser_type`、`browser_read_text` 等），每个工具调用产生的中间数据（HTML 结构、Accessibility Tree 快照、Base64 截图）会直接进入主 Agent 的 Context Window。

**量化影响**：
- 一次典型的浏览器任务（搜索 → 打开页面 → 提取信息）需要 5-15 轮工具调用
- 单轮 `browser_read_text` 返回可达 3K-15K tokens
- `browser_visual_analyze` 的截图 Base64 约 50K-200K tokens
- 累计下来，一次浏览器任务可消耗主 Agent **10%-30%** 的 Context Window

### 1.2 目标

引入 **Agent as Tool** 架构，将浏览器等重上下文领域封装为独立的子智能体：

| 目标 | 衡量标准 |
|------|---------|
| 上下文隔离 | 子 Agent 中间数据不进入主 Agent 上下文 |
| Token 成本降低 | 同任务总 Token 消耗降低 30%+ |
| 黑盒化调用 | 主 Agent 仅看到输入参数 + 最终结果文本 |
| 可配置性 | 子 Agent 的行为（提示词、工具集、模型）完全通过 JSON 配置驱动 |
| 可扩展性 | 框架支持未来添加新类型的子 Agent（如代码审查、数据分析等） |

---

## 2. 核心设计理念

### 2.1 Agent as Tool

子智能体对主 Agent 表现为一个**工具（Tool）**：

```
主 Agent: "我需要搜索 Rust async runtime 的最新进展"
    ↓ 调用工具
call_sub_agent(agent_id: "browser-agent", task: "搜索 Rust async runtime 最新进展，总结 2025-2026 年的关键发展")
    ↓ 黑盒执行
子 Agent 内部: open_url → read_text → click → read_text → ... (5-15 轮)
    ↓ 返回结果
主 Agent 收到: "根据搜索结果，2025-2026 年 Rust async runtime 的主要进展包括：..."
```

### 2.2 三大原则

| 原则 | 说明 |
|------|------|
| **黑盒化执行** | 子 Agent 的内部工具调用链对主 Agent 完全透明，主 Agent 只关注输入和输出 |
| **上下文隔离** | 子 Agent 拥有独立的 ChatMessage 历史和独立的 ToolRegistry，中间状态不污染主 Agent |
| **可序列化** | 子 Agent 的全部配置（提示词、工具集、模型参数）均为 JSON，支持持久化和动态加载 |

---

## 3. 架构总览

### 3.1 交互流程图

```
┌─────────────────────────────────────────────────────────────────┐
│                        主 Agent (Dispatcher)                     │
│                                                                  │
│  System Prompt 中注入子 Agent 描述 → LLM 决策调用 call_sub_agent │
│                                                                  │
│  ┌──────────────────┐                                           │
│  │ ToolRegistry      │  包含:                                    │
│  │                   │  - builtin tools (filesystem, shell...)   │
│  │                   │  - delegation tools (dispatch_claude...)  │
│  │                   │  - planning tools                         │
│  │                   │  - ★ SubAgentTool (新增)                  │
│  └────────┬─────────┘                                           │
│           │                                                      │
│           ▼                                                      │
│  ┌──────────────────────────────────────────────────────┐       │
│  │            SubAgentTool.execute(args, context)        │       │
│  │                                                        │       │
│  │  1. 解析 agent_id + task                              │       │
│  │  2. 查找 SubAgentManager 获取配置                     │       │
│  │  3. 实例化 SubAgentRuntime                            │       │
│  └────────────────────┬─────────────────────────────────┘       │
│                       │                                          │
└───────────────────────┼──────────────────────────────────────────┘
                        │
                        ▼
┌───────────────────────────────────────────────────────────────────┐
│                    SubAgentManager (新增模块)                       │
│                                                                    │
│  ┌─────────────────┐   ┌──────────────────────┐                   │
│  │ DB: sub_agents   │   │ 配置缓存              │                   │
│  │ 表 (SQLite)      │   │ HashMap<agent_id,    │                   │
│  │                  │   │   SubAgentConfig>     │                   │
│  └────────┬────────┘   └──────────┬───────────┘                   │
│           │                       │                                │
│           ▼                       ▼                                │
│  ┌───────────────────────────────────────────────────────────┐    │
│  │           SubAgentConfig (JSON 配置)                       │    │
│  │                                                            │    │
│  │  agent_id: "browser-agent"                                 │    │
│  │  agent_name: "浏览器助手"                                  │    │
│  │  system_prompt: "你是浏览器自动化专家..."                   │    │
│  │  allowed_tools: ["browser_open_url", "browser_click"...]   │    │
│  │  model_config: { ... }                                     │    │
│  └────────────────────────────┬──────────────────────────────┘    │
│                                │                                   │
└────────────────────────────────┼───────────────────────────────────┘
                                 │
                                 ▼
┌────────────────────────────────────────────────────────────────────┐
│                  SubAgentRuntime (新增模块)                          │
│                                                                     │
│  独立的执行上下文:                                                   │
│  ┌──────────────────┐  ┌──────────────────┐  ┌──────────────────┐ │
│  │ 独立 ChatMessage │  │ 独立 ToolRegistry │  │ 独立 LLM Provider│ │
│  │ 历史 (Vec)       │  │ (仅含配置的工具)  │  │ (可独立配置模型) │ │
│  └────────┬─────────┘  └────────┬─────────┘  └────────┬─────────┘ │
│           │                     │                      │           │
│           ▼                     ▼                      │           │
│  ┌──────────────────────────────────────────────┐      │           │
│  │           Agent Loop (复用现有逻辑)           │      │           │
│  │                                               │      │           │
│  │  for iteration in 0..max_iterations {         │      │           │
│  │    response = llm.chat(messages, tools) ◄─────┼──────┘           │
│  │    if response.has_tool_calls() {             │                  │
│  │      for call in response.tool_calls {        │                  │
│  │        result = tools.execute(call)           │                  │
│  │        messages.push(tool_result)             │                  │
│  │      }                                        │                  │
│  │    } else {                                   │                  │
│  │      return response.content  ← 最终结果      │                  │
│  │    }                                          │                  │
│  │  }                                            │                  │
│  └───────────────────────────────────────────────┘                  │
│                                                                      │
│  执行期间:                                                           │
│  - 中间 tool call 事件通过 SubAgentEvent 通知前端 (可选展示)         │
│  - 中间数据**不**写入主 Agent 的 messages 历史                       │
│  - 最终结果文本返回给主 Agent 的 tool_results                        │
│                                                                      │
└──────────────────────────────────────────────────────────────────────┘
```

### 3.2 新增模块清单

| 模块 | 路径 | 职责 |
|------|------|------|
| `sub_agent/mod.rs` | `src-tauri/src/agent/sub_agent/` | 模块入口 |
| `sub_agent/config.rs` | 同上 | `SubAgentConfig` 数据结构定义 + JSON Schema |
| `sub_agent/manager.rs` | 同上 | `SubAgentManager`：配置 CRUD + 缓存管理 |
| `sub_agent/runtime.rs` | 同上 | `SubAgentRuntime`：独立 Agent Loop 执行引擎 |
| `sub_agent/tool.rs` | 同上 | `SubAgentTool`：实现 `AgentTool` trait，作为主 Agent 可调用的工具 |
| `sub_agent/db.rs` | 同上 | SQLite 表操作：`sub_agents` 表的 CRUD |
| `sub_agent/commands.rs` | 同上 | Tauri 命令：前端 CRUD API + 关联配置 |

### 3.3 与现有架构的关系

```
现有架构:
  DispatcherAgent
    └── ToolRegistry
          ├── builtin tools (13 个)
          ├── delegation tools (6 个)
          ├── planning tools (8 个)
          └── MCP dynamic tools

新增后:
  DispatcherAgent
    └── ToolRegistry
          ├── builtin tools (13 个，浏览器工具保留但默认不暴露给主 Agent)
          ├── delegation tools (6 个)
          ├── planning tools (8 个)
          ├── MCP dynamic tools
          └── ★ call_sub_agent (1 个，动态注册)
```

---

## 4. 数据模型设计

### 4.1 SubAgentConfig JSON Schema

```json
{
  "$schema": "http://json-schema.org/draft-07/schema#",
  "title": "SubAgentConfig",
  "description": "子智能体的完整配置定义",
  "type": "object",
  "required": ["agent_id", "agent_name", "description", "system_prompt", "allowed_tools"],
  "properties": {
    "agent_id": {
      "type": "string",
      "pattern": "^[a-z0-9][a-z0-9_-]{0,63}$",
      "description": "唯一标识符，仅支持小写字母、数字、下划线和短横线"
    },
    "agent_name": {
      "type": "string",
      "maxLength": 64,
      "description": "显示名称，如「浏览器助手」"
    },
    "description": {
      "type": "string",
      "maxLength": 512,
      "description": "功能描述，注入主 Agent 的 System Prompt 供 LLM 路由选择"
    },
    "system_prompt": {
      "type": "string",
      "description": "子 Agent 的系统指令，定义其角色、行为边界和输出格式"
    },
    "user_prompt_template": {
      "type": "string",
      "description": "用户输入模板，支持 {{task}} 占位符；为空时直接使用 task 原文",
      "default": "{{task}}"
    },
    "allowed_tools": {
      "type": "array",
      "items": {
        "type": "string",
        "enum": [
          "browser_open_url",
          "browser_click",
          "browser_type",
          "browser_press",
          "browser_wait_for",
          "browser_read_text",
          "browser_visual_analyze",
          "browser_close",
          "read_file",
          "write_file",
          "edit_file",
          "list_dir",
          "glob",
          "grep",
          "exec",
          "generate_image",
          "edit_image"
        ]
      },
      "minItems": 1,
      "description": "子 Agent 可使用的工具名称列表，必须取自系统已实现的 AgentTool 实现类"
    },
    "model_config": {
      "type": "object",
      "properties": {
        "inherit_from_parent": {
          "type": "boolean",
          "description": "是否继承主 Agent 的 LLM Provider 配置",
          "default": true
        },
        "api_base": {
          "type": "string",
          "description": "自定义 API Base URL（inherit_from_parent=false 时使用）"
        },
        "api_key": {
          "type": "string",
          "description": "自定义 API Key（inherit_from_parent=false 时使用）"
        },
        "model_name": {
          "type": "string",
          "description": "模型名称；为空时使用主 Agent 同类型模型"
        }
      },
      "default": { "inherit_from_parent": true }
    },
    "max_iterations": {
      "type": "integer",
      "minimum": 1,
      "maximum": 100,
      "description": "子 Agent 单次执行的最大 LLM 迭代轮次",
      "default": 20
    },
    "max_output_tokens": {
      "type": "integer",
      "minimum": 256,
      "maximum": 65536,
      "description": "子 Agent 单次 LLM 调用的 max_tokens",
      "default": 4096
    },
    "temperature": {
      "type": "number",
      "minimum": 0,
      "maximum": 2,
      "description": "LLM temperature 参数",
      "default": 0.7
    },
    "timeout_secs": {
      "type": "integer",
      "minimum": 10,
      "maximum": 600,
      "description": "子 Agent 单次执行的超时时间（秒）",
      "default": 120
    },
    "enabled": {
      "type": "boolean",
      "description": "是否启用，禁用后不出现在主 Agent 可用工具中",
      "default": true
    },
    "created_at": {
      "type": "integer",
      "description": "创建时间戳（毫秒）"
    },
    "updated_at": {
      "type": "integer",
      "description": "最后更新时间戳（毫秒）"
    }
  }
}
```

### 4.2 数据库表设计

```sql
CREATE TABLE IF NOT EXISTS sub_agents (
    id          TEXT PRIMARY KEY,                    -- agent_id
    name        TEXT NOT NULL,                       -- agent_name
    description TEXT NOT NULL,                       -- 功能描述
    config_json TEXT NOT NULL,                       -- 完整 SubAgentConfig JSON
    enabled     INTEGER NOT NULL DEFAULT 1,          -- 0/1
    created_at  INTEGER NOT NULL,                    -- Unix 毫秒时间戳
    updated_at  INTEGER NOT NULL                     -- Unix 毫秒时间戳
);

-- 主 Agent ↔ 子 Agent 关联表
CREATE TABLE IF NOT EXISTS session_sub_agents (
    session_id  TEXT NOT NULL,                       -- dispatcher_sessions.id
    sub_agent_id TEXT NOT NULL,                      -- sub_agents.id
    PRIMARY KEY (session_id, sub_agent_id),
    FOREIGN KEY (session_id) REFERENCES dispatcher_sessions(id) ON DELETE CASCADE,
    FOREIGN KEY (sub_agent_id) REFERENCES sub_agents(id) ON DELETE CASCADE
);

-- 全局关联（所有会话默认启用）
CREATE TABLE IF NOT EXISTS global_sub_agents (
    sub_agent_id TEXT PRIMARY KEY,
    FOREIGN KEY (sub_agent_id) REFERENCES sub_agents(id) ON DELETE CASCADE
);
```

### 4.3 Rust 数据结构

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubAgentConfig {
    pub agent_id: String,
    pub agent_name: String,
    pub description: String,
    pub system_prompt: String,
    pub user_prompt_template: String,
    pub allowed_tools: Vec<String>,
    pub model_config: SubAgentModelConfig,
    pub max_iterations: u32,
    pub max_output_tokens: u32,
    pub temperature: f64,
    pub timeout_secs: u64,
    pub enabled: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubAgentModelConfig {
    pub inherit_from_parent: bool,
    pub api_base: Option<String>,
    pub api_key: Option<String>,
    pub model_name: Option<String>,
}

// 数据库记录
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubAgentRecord {
    pub id: String,
    pub name: String,
    pub description: String,
    pub config_json: String,
    pub enabled: bool,
    pub created_at: i64,
    pub updated_at: i64,
}
```

---

## 5. 后端：Sub-Agent Runtime 框架

### 5.1 SubAgentManager

负责子 Agent 配置的生命周期管理。

```rust
pub struct SubAgentManager {
    db: Arc<DispatcherDb>,
    cache: RwLock<HashMap<String, SubAgentConfig>>,
}

impl SubAgentManager {
    /// 从 DB 加载全部配置到缓存
    pub async fn load_all(&self) -> Result<Vec<SubAgentConfig>>;

    /// 获取单个配置
    pub fn get(&self, agent_id: &str) -> Option<SubAgentConfig>;

    /// 创建新子 Agent
    pub async fn create(&self, config: SubAgentConfig) -> Result<()>;

    /// 更新子 Agent 配置
    pub async fn update(&self, config: SubAgentConfig) -> Result<()>;

    /// 删除子 Agent
    pub async fn delete(&self, agent_id: &str) -> Result<()>;

    /// 获取某会话关联的全部启用子 Agent
    pub async fn get_enabled_for_session(&self, session_id: &str) -> Vec<SubAgentConfig>;

    /// 获取全局启用的子 Agent
    pub async fn get_global_enabled(&self) -> Vec<SubAgentConfig>;
}
```

### 5.2 SubAgentRuntime

独立执行引擎，复用现有 `DispatcherAgent` 的核心 Agent Loop 逻辑。

```rust
pub struct SubAgentRuntime {
    config: SubAgentConfig,
    provider: OpenAiCompatProvider,
    tool_registry: ToolRegistry,
}

impl SubAgentRuntime {
    /// 从 SubAgentConfig 构建 Runtime 实例
    /// - 根据 allowed_tools 从全局 ToolRegistry 中筛选子集
    /// - 根据 model_config 决定使用主 Agent 的 provider 还是独立 provider
    pub fn build(
        config: &SubAgentConfig,
        parent_provider: &OpenAiCompatProvider,
        parent_tools: &ToolRegistry,
        tool_context: &ToolContext,
    ) -> Result<Self>;

    /// 执行子 Agent 任务，返回最终结果文本
    pub async fn execute(
        &self,
        task: &str,
        event_sender: Option<mpsc::Sender<SubAgentEvent>>,
    ) -> Result<String>;
}
```

**核心执行逻辑**（伪代码）：

```rust
pub async fn execute(&self, task: &str, event_sender: ...) -> Result<String> {
    let user_prompt = self.config.user_prompt_template
        .replace("{{task}}", task);

    let mut messages = vec![
        ChatMessage::system(self.config.system_prompt.clone()),
        ChatMessage::user(user_prompt),
    ];
    let tools = self.tool_registry.definitions_for_workspace(
        &context.workspace, self.config.allowed_tools.iter().map(|s| s.as_str()), false
    );

    for iteration in 0..self.config.max_iterations {
        // 检查超时
        if start.elapsed() > Duration::from_secs(self.config.timeout_secs) {
            return Err(anyhow!("子 Agent 执行超时"));
        }

        let response = self.provider.chat_stream(
            &messages, &tools, false, |delta| { /* 可选：事件通知前端 */ }
        ).await?;

        // 无 tool_calls → 最终结果
        if response.tool_calls.is_empty() {
            return Ok(response.content);
        }

        // 处理 tool calls
        messages.push(assistant_message_with_tool_calls(&response));
        for call in &response.tool_calls {
            let result = self.tool_registry.execute(&call.name, &call.arguments, &context).await;
            messages.push(tool_result_message(call.id, &result));
        }
    }

    Err(anyhow!("子 Agent 达到最大迭代次数"))
}
```

### 5.3 SubAgentTool

实现 `AgentTool` trait，作为主 Agent 工具注册的入口。

```rust
pub struct SubAgentTool {
    manager: Arc<SubAgentManager>,
}

#[async_trait]
impl AgentTool for SubAgentTool {
    fn name(&self) -> &'static str {
        "call_sub_agent"
    }

    fn description(&self) -> &'static str {
        "调用一个子智能体执行特定领域的复杂任务。子智能体拥有独立的执行上下文，内部的工具调用过程对你透明，你只会收到最终结果。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "agent_id": {
                    "type": "string",
                    "description": "子智能体的 ID。通过 list_sub_agents 查看可用列表。"
                },
                "task": {
                    "type": "string",
                    "description": "要交给子智能体的任务描述，应清晰说明期望的行为和输出格式。"
                }
            },
            "required": ["agent_id", "task"]
        })
    }

    async fn execute(&self, args: &Value, context: &ToolContext) -> String {
        let agent_id = string_arg(args, "agent_id").unwrap_or_default();
        let task = string_arg(args, "task").unwrap_or_default();

        // 1. 获取子 Agent 配置
        let config = match self.manager.get(&agent_id) {
            Some(c) if c.enabled => c,
            Some(_) => return format!("错误：子智能体 '{}' 已被禁用", agent_id),
            None => return format!("错误：未找到子智能体 '{}'", agent_id),
        };

        // 2. 构建 Runtime 并执行
        let runtime = SubAgentRuntime::build(&config, ...);
        match runtime.execute(&task, event_sender).await {
            Ok(result) => result,
            Err(e) => format!("子智能体执行失败：{}", e),
        }
    }
}
```

### 5.4 工具注册机制

子 Agent 的 `allowed_tools` 中的工具名称必须取自系统已实现的 `AgentTool` 实现类。系统在初始化时收集所有已注册工具的 `name()`，供配置时校验。

新增辅助工具 `list_sub_agents`，让主 Agent 能动态发现可用的子 Agent：

```rust
pub struct ListSubAgentsTool {
    manager: Arc<SubAgentManager>,
}

#[async_trait]
impl AgentTool for ListSubAgentsTool {
    fn name(&self) -> &'static str { "list_sub_agents" }
    fn description(&self) -> &'static str {
        "列出当前可用的全部子智能体及其描述，帮助你决定调用哪个子智能体。"
    }
    fn parameters(&self) -> Value {
        json!({ "type": "object", "properties": {} })
    }
    async fn execute(&self, _args: &Value, _context: &ToolContext) -> String {
        // 返回所有 enabled 子 Agent 的 id + name + description
    }
}
```

---

## 6. 接口设计

### 6.1 主 Agent → Sub-Agent 调用协议

**工具名称**: `call_sub_agent`

**输入参数**:
```json
{
  "agent_id": "browser-agent",
  "task": "在 GitHub 上搜索 tokio-rs/tokio 仓库最近的 release，总结 v1.x 的最新三个版本的主要变更"
}
```

**输出**: 纯文本结果字符串

```
根据搜索结果，tokio v1.x 的最新三个版本及其主要变更：

1. **v1.42.0** (2024-12):
   - 新增 `JoinSet::join_next` API
   - 优化了 IO 驱动调度...

2. **v1.41.0** (2024-10):
   - 修复了 timer wheel 的竞态条件...

3. **v1.40.0** (2024-08):
   - 新增 `Runtime::metrics` API...
```

### 6.2 Tauri 命令（前端 CRUD API）

| 命令名 | 参数 | 返回 | 说明 |
|--------|------|------|------|
| `sub_agent_list` | - | `Vec<SubAgentRecord>` | 列出所有子 Agent |
| `sub_agent_get` | `id: String` | `SubAgentRecord` | 获取单个配置 |
| `sub_agent_create` | `config_json: String` | `SubAgentRecord` | 创建 |
| `sub_agent_update` | `id: String, config_json: String` | `SubAgentRecord` | 更新 |
| `sub_agent_delete` | `id: String` | `()` | 删除 |
| `sub_agent_list_tools` | - | `Vec<ToolInfo>` | 列出系统所有可用工具 |
| `sub_agent_set_session_enabled` | `session_id, sub_agent_ids: Vec<String>` | `()` | 设置会话级关联 |
| `sub_agent_get_session_enabled` | `session_id` | `Vec<SubAgentRecord>` | 获取会话关联的子 Agent |
| `sub_agent_set_global_enabled` | `sub_agent_ids: Vec<String>` | `()` | 设置全局关联 |
| `sub_agent_get_global_enabled` | - | `Vec<SubAgentRecord>` | 获取全局关联 |

### 6.3 SubAgentEvent（前端实时状态通知）

子 Agent 执行期间，通过 Tauri 事件系统向前端推送状态：

```rust
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "event", content = "data")]
pub enum SubAgentEvent {
    Started {
        agent_id: String,
        task: String,
    },
    ToolStarted {
        tool_name: String,
        arguments: Value,
    },
    ToolFinished {
        tool_name: String,
        result_preview: String,
    },
    LlmDelta {
        delta: String,
    },
    Finished {
        result: String,
        iterations: u32,
        elapsed_ms: u64,
        token_usage: SubAgentUsage,
    },
    Failed {
        error: String,
    },
}
```

前端事件名: `sub-agent-event`，payload 包含 `session_id` 和 `SubAgentEvent`。

---

## 7. 前端：管理界面设计

### 7.1 入口位置

在现有 `AppSettingsDialog` 的导航栏中新增一个 **子智能体** 标签页：

```
AppSettingsDialog 导航:
  常规 | 主题 | Aha 智能体 | 子智能体 ★ (新增) | Claude Code | Codex
```

### 7.2 子 Agent 管理页 - SubAgentPanel

```
┌────────────────────────────────────────────────────────────────┐
│  子智能体管理                                       [+ 新建]    │
├────────────────────────────────────────────────────────────────┤
│                                                                 │
│  ┌──────────────────────────────────────────────────────────┐  │
│  │ 🟢 浏览器助手 (browser-agent)                            │  │
│  │    处理网页搜索、页面自动化和信息提取任务                  │  │
│  │    工具: browser_open_url, browser_click, ... (8个)      │  │
│  │                                          [编辑] [删除]    │  │
│  ├──────────────────────────────────────────────────────────┤  │
│  │ 🟢 代码搜索助手 (code-search-agent)                      │  │
│  │    在代码库中定位和分析特定模式                            │  │
│  │    工具: glob, grep, read_file (3个)                      │  │
│  │                                          [编辑] [删除]    │  │
│  └──────────────────────────────────────────────────────────┘  │
│                                                                 │
└────────────────────────────────────────────────────────────────┘
```

### 7.3 子 Agent 编辑对话框 - SubAgentEditorDialog

点击 "编辑" 或 "新建" 时弹出的对话框，含三个 Tab：

**Tab 1: 基本信息**

```
┌─────────────────────────────────────────────────────┐
│  基本信息                                            │
├─────────────────────────────────────────────────────┤
│                                                      │
│  Agent ID:     [browser-agent_________] (新建时可编辑)│
│  显示名称:     [浏览器助手___________]                │
│  功能描述:     [处理网页搜索、页面自动化...]           │
│                                                      │
│  系统指令 (System Prompt):                           │
│  ┌─────────────────────────────────────────────────┐│
│  │ 你是一个浏览器自动化专家。你的任务是：            ││
│  │ 1. 根据用户的指令打开网页并提取信息               ││
│  │ 2. 执行页面交互操作（点击、输入、滚动）           ││
│  │ ...                                             ││
│  └─────────────────────────────────────────────────┘│
│                                                      │
│  用户输入模板:  [{{task}}_____________]               │
│                                                      │
└─────────────────────────────────────────────────────┘
```

**Tab 2: 工具集配置**

```
┌─────────────────────────────────────────────────────┐
│  工具集配置                                          │
├─────────────────────────────────────────────────────┤
│                                                      │
│  已选工具 (8):                                       │
│  [x] browser_open_url    - 打开网页                  │
│  [x] browser_click       - 点击元素                  │
│  [x] browser_type        - 输入文本                  │
│  [x] browser_press       - 按键                      │
│  [x] browser_wait_for    - 等待页面状态              │
│  [x] browser_read_text   - 读取可访问性树            │
│  [x] browser_visual_analyze - 视觉分析               │
│  [x] browser_close       - 关闭浏览器                │
│                                                      │
│  可选工具:                                           │
│  [ ] read_file           - 读取文件                  │
│  [ ] write_file          - 写入文件                  │
│  [ ] edit_file           - 编辑文件                  │
│  [ ] list_dir            - 列出目录                  │
│  [ ] glob                - 文件模式搜索              │
│  [ ] grep                - 内容正则搜索              │
│  [ ] exec                - 执行 Shell 命令           │
│  [ ] generate_image      - 生成图片                  │
│  [ ] edit_image          - 编辑图片                  │
│                                                      │
└─────────────────────────────────────────────────────┘
```

> 工具列表通过 `sub_agent_list_tools` 命令从后端获取，动态展示所有已实现的 `AgentTool` 实现类，前端仅呈现 checkbox 选择器。

**Tab 3: 运行时参数**

```
┌─────────────────────────────────────────────────────┐
│  运行时参数                                          │
├─────────────────────────────────────────────────────┤
│                                                      │
│  模型配置:                                           │
│  ○ 继承主 Agent 配置                                 │
│  ○ 自定义配置                                        │
│    API Base:  [________________________]             │
│    API Key:   [________________________]             │
│    模型名称:  [qwen-max________________]             │
│                                                      │
│  最大迭代轮次:   [20___]  (1-100)                    │
│  最大输出 Token: [4096_]  (256-65536)               │
│  Temperature:    [0.7__]  (0-2)                     │
│  超时时间(秒):   [120__]  (10-600)                   │
│                                                      │
└─────────────────────────────────────────────────────┘
```

### 7.4 主 Agent 关联配置

在 Aha 智能体设置的 `AhaAgentPanel` 中新增一个 **"关联子智能体"** 区域：

```
┌────────────────────────────────────────────────────────────────┐
│  Aha 智能体设置                                                │
├────────────────────────────────────────────────────────────────┤
│  [Chat 模型] [Vision 模型] [Image 模型] [语音] [Embedding]     │
│                                                                │
│  ...现有模型配置 UI...                                         │
│                                                                │
│  ───────── 关联子智能体 ─────────                              │
│                                                                │
│  全局启用的子智能体（所有会话默认可用）：                        │
│  [x] 🟢 浏览器助手 (browser-agent)                            │
│  [ ] 代码搜索助手 (code-search-agent)                          │
│  [ ] 数据分析助手 (data-analysis-agent)                        │
│                                                                │
│  行为设置:                                                     │
│  [x] 自动审批 Dispatch                                        │
│  [ ] 上下文调试                                                │
│                                                                │
└────────────────────────────────────────────────────────────────┘
```

### 7.5 运行时 UI：Sub-Agent 执行状态展示

在 `DispatcherChat` 的消息流中，当主 Agent 调用 `call_sub_agent` 时，工具执行区域展示折叠面板：

```
┌─ 🤖 子智能体: 浏览器助手 ──────────────── 执行中 (12s) ─┐
│                                                         │
│  任务: 搜索 tokio 最新 release                          │
│                                                         │
│  ▶ LLM 回复: 正在分析页面元素...                        │
│  ▶ browser_open_url("https://github.com/tokio-rs...")  │
│  ▶ browser_read_text()                                  │
│  ▶ browser_click(ref="r42")                             │
│  ▶ browser_read_text()                                  │
│  ...                                                    │
│                                                         │
│  [折叠/展开详情]                                        │
└─────────────────────────────────────────────────────────┘
```

对应前端新增组件: **`SubAgentExecutionView`**，监听 `sub-agent-event` 事件渲染实时状态。

---

## 8. 浏览器子智能体专项设计

### 8.1 初始配置

系统初始化时自动创建以下内置子 Agent 配置：

```json
{
  "agent_id": "browser-agent",
  "agent_name": "浏览器助手",
  "description": "处理网页搜索、页面浏览、信息提取和页面自动化任务。适合需要从互联网获取信息的场景，如搜索文档、查询 API、阅读网页内容、填写表单等。",
  "system_prompt": "你是一个专业的浏览器自动化助手。你通过 CloakBrowser 工具与网页交互。\n\n## 工作流程\n1. 使用 browser_read_text 获取页面可访问性树快照，获取元素 ref\n2. 使用 ref 与具体元素交互（click, type）\n3. 使用 browser_visual_analyze 进行视觉分析（仅在文本信息不足时使用）\n\n## 输出规范\n- 完成任务后，输出结构化的信息提取结果\n- 如果搜索未找到结果，明确说明并建议替代方案\n- 不要输出原始的 HTML 或可访问性树内容\n\n## 约束\n- 不要访问需要登录的页面，除非用户明确指示\n- 每次操作后验证页面状态\n- 遇到验证码或反爬机制时，通知用户",
  "user_prompt_template": "{{task}}",
  "allowed_tools": [
    "browser_open_url",
    "browser_click",
    "browser_type",
    "browser_press",
    "browser_wait_for",
    "browser_read_text",
    "browser_visual_analyze",
    "browser_close"
  ],
  "model_config": {
    "inherit_from_parent": true
  },
  "max_iterations": 25,
  "max_output_tokens": 4096,
  "temperature": 0.7,
  "timeout_secs": 180,
  "enabled": true
}
```

### 8.2 工具迁移方案

当前主 Agent 直接使用的 8 个浏览器工具（`builtin/browser.rs`）保持不变，但改变其对主 Agent 的暴露方式：

```
迁移前:
  DispatcherAgent ToolRegistry
    └── browser_tools() → 8 个浏览器工具全部注册

迁移后:
  DispatcherAgent ToolRegistry
    └── browser_tools() → 8 个浏览器工具仍然注册在全局 Registry 中
                           但主 Agent 的 allowed 列表中默认排除它们
                           仅通过 sub_agent_config.allowed_tools 授权给子 Agent

  SubAgentRuntime("browser-agent") ToolRegistry
    └── 从全局 Registry 中按 allowed_tools 筛选出 8 个浏览器工具的子集
```

**关键点**：工具实现代码 **不移动、不复制**。`SubAgentRuntime` 通过 `allowed_tools` 参数从全局 `ToolRegistry` 中过滤出需要的工具，共享同一个 `AgentTool` 实例。

### 8.3 主 Agent System Prompt 变更

迁移后，主 Agent 的 System Prompt 中将不再列出 8 个浏览器工具的详细参数说明，取而代之的是子 Agent 的描述：

**移除**：
```
## browser_open_url
使用项目级 CloakBrowser 打开网页...
## browser_click
点击 Accessibility Tree 快照中的元素 ref...
... (8 个工具的描述)
```

**替换为**：
```
## 可用的子智能体

你可以使用 call_sub_agent 工具调用以下子智能体：

### browser-agent（浏览器助手）
处理网页搜索、页面浏览、信息提取和页面自动化任务。
适合需要从互联网获取信息的场景，如搜索文档、查询 API、阅读网页内容、填写表单等。

示例调用：
call_sub_agent(agent_id="browser-agent", task="在 Google 搜索 'Rust async runtime'，总结前 3 条结果")
```

### 8.4 执行时序图

```
主 Agent (Dispatcher)                SubAgentRuntime (Browser Agent)         CloakBrowser
      │                                        │                                │
      │ call_sub_agent(                        │                                │
      │   agent_id="browser-agent",            │                                │
      │   task="搜索 tokio release")          │                                │
      │ ─────────────────────────────────────►│                                │
      │                                        │                                │
      │                                        │ browser_open_url(              │
      │                                        │   "https://github.com/to...")  │
      │                                        │ ──────────────────────────────►│
      │                                        │                                │ 打开页面
      │                                        │◄──────────────────────────────│
      │                                        │ {success, page_title, ...}     │
      │                                        │                                │
      │                                        │ browser_read_text()            │
      │                                        │ ──────────────────────────────►│
      │                                        │◄──────────────────────────────│
      │                                        │ "ref r1: link 'Releases'..."   │
      │                                        │                                │
      │                                        │ browser_click(ref="r1")        │
      │                                        │ ──────────────────────────────►│
      │                                        │◄──────────────────────────────│
      │                                        │                                │
      │                                        │ browser_read_text()            │
      │                                        │ ──────────────────────────────►│
      │                                        │◄──────────────────────────────│
      │                                        │ "v1.42.0, v1.41.0, ..."        │
      │                                        │                                │
      │                                        │ [LLM 整理结果，发现无需继续]   │
      │                                        │                                │
      │◄──────────────────────────────────────│                                │
      │ "根据搜索结果，tokio 最新版本..."      │                                │
      │                                        │                                │
      │ [主 Agent 将结果纳入自己的上下文]      │                                │
      │ [主 Agent 的 messages 中只增加了       │                                │
      │  1 条 tool_result 消息]                │                                │
```

**上下文对比**：

| | 迁移前（主 Agent 直接使用浏览器工具） | 迁移后（通过子 Agent） |
|---|---|---|
| 主 Agent messages 增加 | 10-15 条 tool_call + tool_result | 1 条 tool_call + 1 条 tool_result |
| 主 Agent 上下文增量 | 30K-100K tokens | 1K-3K tokens |
| 中间数据（HTML 快照等） | 全部进入主 Agent 上下文 | 仅在子 Agent 内部存在，执行后销毁 |

---

## 9. 迁移与兼容方案

### 9.1 数据库迁移

在 `db.rs` 中添加新 migration 步骤：
- 创建 `sub_agents`、`session_sub_agents`、`global_sub_agents` 表
- 插入默认 `browser-agent` 配置

### 9.2 向后兼容

- **浏览器工具保留**：8 个浏览器工具的 `AgentTool` 实现不变，仍在全局 `ToolRegistry` 中注册
- **渐进式迁移**：通过 Session 级配置决定是否将浏览器工具暴露给主 Agent
  - 默认：浏览器工具不暴露给主 Agent，仅通过子 Agent 调用
  - 用户可在会话设置中手动添加浏览器工具到主 Agent（回退兼容）

### 9.3 前端兼容

- `BrowserPanel`（CloakBrowser 右侧面板）保持不变——它监听的是 `BrowserManager` 事件，不关心调用来源
- 子 Agent 的浏览器操作同样会触发 `browser-frame`、`browser-status` 事件

---

## 10. 实施路线

### Phase 1: 后端框架（预估 3-4 天）

1. 创建 `sub_agent/` 模块目录结构
2. 实现 `SubAgentConfig` 数据结构 + 验证
3. 添加 `sub_agents` 等数据库表 + migration
4. 实现 `SubAgentManager`（配置 CRUD + 缓存）
5. 实现 `SubAgentRuntime`（独立 Agent Loop）
6. 实现 `SubAgentTool` + `ListSubAgentsTool`
7. 在主 Agent 的 `ToolRegistry` 构建流程中注册

### Phase 2: Tauri 命令 + 前端管理 UI（预估 3-4 天）

1. 实现 CRUD Tauri 命令（`sub_agent_list`, `sub_agent_create` 等）
2. 实现关联配置命令（`sub_agent_set_session_enabled` 等）
3. 前端：`SubAgentPanel`（管理列表页）
4. 前端：`SubAgentEditorDialog`（新建/编辑对话框）
5. 前端：`AhaAgentPanel` 中新增关联配置区域

### Phase 3: 浏览器子 Agent 迁移（预估 2 天）

1. 配置系统初始化 `browser-agent`
2. 调整主 Agent 的 tool allowed 列表（默认排除浏览器工具）
3. 修改 `build_system_prompt` 注入子 Agent 描述
4. 前端 `SubAgentExecutionView` 组件

### Phase 4: 验证与优化（预估 1-2 天）

1. 端到端测试：主 Agent 调用浏览器子 Agent 完成搜索任务
2. 对比测试：迁移前后同任务的 Token 消耗对比
3. 超时和错误处理边界条件
4. 前端事件流的实时更新和折叠展示
