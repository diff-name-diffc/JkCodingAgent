# 消息图片存储与多模态检索改造执行计划

> 目标：将当前纯文本消息改造为支持有序文本+图片片段的消息模型，为后续多模态向量检索（LanceDB）奠定数据基础。
>
> **原则：不考虑向后兼容，`dispatcher_messages` 移除 `content` 字段，改为 `segments_json` JSON 数组单字段，消除数据冗余。**

---

## 一、当前架构诊断

### 1.1 数据模型现状

```
┌─────────────────────────────────────────────────────────────┐
│                    DispatcherMessage (TS)                    │
├─────────────────────────────────────────────────────────────┤
│  id, workspaceId, role, content: string, createdAt...       │
│  无 media / image 字段                                        │
└─────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────┐
│               dispatcher_messages (SQLite)                 │
├─────────────────────────────────────────────────────────────┤
│  id | workspace_id | role | content TEXT | created_at       │
│  单条 content 存储完整 Markdown 文本（含内嵌图片 base64）    │
└─────────────────────────────────────────────────────────────┘
```

### 1.2 图片处理现状

- **粘贴**：`handlePaste` → `FileReader.readAsDataURL()` → base64 data URL
- **发送**：`sendUserMessage` 将图片拼接为 `![image](data:image/...)` 前缀，合并到 `content`
- **渲染**：`MarkdownRenderer` 通过 `react-markdown` 解析 Markdown 中的 `img` 标签
- **问题**：
  - 图片丢失位置信息（始终在文本最前面）
  - base64 直接存储在 content 中，消息体臃肿
  - 无法单独检索图片
  - 无图片元数据（尺寸、文件名、生成参数等）
  - 不支持图片生成工具的输出

---

## 二、改造目标

### 2.1 核心目标

| # | 目标 | 优先级 |
|---|------|--------|
| 1 | `dispatcher_messages` 移除 `content`，改为 `segments_json` JSON 数组单字段 | P0 |
| 2 | 文件系统按 **聊天会话标题** 隔离存储图片 | P0 |
| 3 | `chat_images` 辅助表存储图片元数据（用于向量检索扩展） | P0 |
| 4 | 渲染/LLM 调用时动态组装 Markdown（`segmentsToMarkdown`） | P0 |
| 5 | 图片生成工具接口设计（暂不实现） | P1 |
| 6 | LanceDB 向量表预留扩展（暂不实现） | P2 |

### 2.2 非目标

- 不向后兼容旧数据（不考虑兼容）
- 不实现图片生成工具本体（标记 TODO）
- 不实现多模态向量检索（预留数据结构）
- 不改写现有文件系统 Projects/Tasks 存储逻辑

---

## 三、数据模型设计

### 3.1 为什么用 `segments_json` 而非拆表

| 对比项 | 拆 `message_segments` 表 | 单 `segments_json` 字段 |
|--------|-------------------------|----------------------|
| 存储冗余 | `content` + `message_segments` 重复 | 只有 `segments_json` 一份 |
| 查询复杂度 | 需要 JOIN 多表 | 单表查询，无需 JOIN |
| 顺序保障 | 依赖 `order_index` 字段 | JSON 数组天然有序 |
| 原子性 | 跨表事务 | 单条记录原子写入 |
| 写入性能 | INSERT 多张表 | 单条 INSERT |
| 扩展灵活度 | 修改表结构 | 扩展 JSON Schema |

### 3.2 TypeScript 类型改造

#### 新增：消息片段类型（`src/types.ts`）

```typescript
// ── Content Segment ──

export type ContentSegmentType = "text" | "image" | "file";

export interface ContentSegment {
  id: string;           // 片段唯一标识 uuid
  type: ContentSegmentType;
  // 各子类型扩展字段
}

export interface TextSegment extends ContentSegment {
  type: "text";
  text: string;         // 纯文本内容
}

export interface ImageSegment extends ContentSegment {
  type: "image";
  imageId: string;      // 图片唯一标识（与文件系统文件名对应）
  path: string;         // 本地路径：asset://chat-images/{session-title}/{imageId}.{ext}
  alt?: string;         // 图片 alt 描述（用于图片搜索）
  width?: number;       // 宽度（px）
  height?: number;      // 高度（px）
  mimeType?: string;    // image/png, image/jpeg...
  source: "user_paste" | "tool_generate" | "file_attach";
  generationPrompt?: string;  // 如果是工具生成，记录生成提示词
}

export interface FileSegment extends ContentSegment {
  type: "file";
  fileId: string;
  path: string;
  fileName: string;
  mimeType: string;
  size: number;         // bytes
}

export type AnyContentSegment = TextSegment | ImageSegment | FileSegment;
```

#### 改造：`DispatcherMessage`（`src/types.ts`）

```typescript
// 改造前
export interface DispatcherMessage {
  id: string;
  workspaceId: string;
  role: "user" | "assistant" | "tool";
  content: string;      // ← 纯文本 Markdown
  // ...
}

// 改造后（彻底移除 content，改为 segments_json）
export interface DispatcherMessage {
  id: string;
  workspaceId: string;
  role: "user" | "assistant" | "tool";
  segments: AnyContentSegment[];  // ← 有序片段数组（JSON 序列化后存储）
  // content 已移除
  // ...
}
```

#### 新增：图片生成工具参数（`src/types.ts`）

```typescript
// ── Image Generation Tool ──
// TODO: 图片生成工具本体暂不实现，先定义接口

export interface ImageGenerationInput {
  prompt: string;           // 生成提示词
  width?: number;           // 可选：宽度
  height?: number;          // 可选：高度
  style?: string;           // 可选：风格
  negativePrompt?: string;  // 可选：反向提示词
  model?: string;           // 可选：模型名称
  seed?: number;            // 可选：随机种子
}

export interface ImageGenerationOutput {
  imageId: string;          // 生成的图片 ID
  path: string;             // 本地文件路径（已通过 asset:// 协议）
  width: number;
  height: number;
  mimeType: string;
  generationPrompt: string; // 实际使用的提示词
  generationParams: Record<string, unknown>;
  createdAt: string;
}
```

---

### 3.3 SQLite 表结构改造

#### 改造：`dispatcher_messages`

```sql
-- 改造后的 dispatcher_messages（移除 content，新增 segments_json）
CREATE TABLE IF NOT EXISTS dispatcher_messages (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL,
    role TEXT NOT NULL,
    segments_json TEXT NOT NULL,        -- 权威：JSON 数组，包含有序文本+图片
    thinking_content TEXT,
    thinking_elapsed_ms INTEGER,
    context_payload TEXT,
    tool_call_id TEXT,
    tool_name TEXT,
    tool_result_mode TEXT,
    tool_artifacts_json TEXT,
    tool_calls_json TEXT,
    usage_stats_json TEXT,
    visible INTEGER NOT NULL DEFAULT 1,
    context_cleared INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_dispatcher_messages_workspace_created
ON dispatcher_messages(workspace_id, created_at);
```

> **注意**：`content` 字段已移除。所有消息内容通过 `segments_json` 存储，渲染/调用 LLM 时动态组装 Markdown。

#### 新增表：`chat_images`
（用于全局图片索引和向量检索扩展）

```sql
-- 全局图片索引表（支持跨会话检索，不存储重复数据）
CREATE TABLE IF NOT EXISTS chat_images (
    id TEXT PRIMARY KEY,
    image_id TEXT NOT NULL UNIQUE,
    workspace_id TEXT NOT NULL,
    message_id TEXT NOT NULL,
    segment_index INTEGER NOT NULL,    -- 片段在 segments_json 中的索引位置
    path TEXT NOT NULL,                -- 本地文件路径
    alt TEXT,
    width INTEGER,
    height INTEGER,
    mime_type TEXT,
    source TEXT,
    generation_prompt TEXT,
    -- 向量检索预留字段（暂不填充）
    vector_embedding_json TEXT,        -- 图片向量（JSON 数组，后续迁移到 LanceDB）
    text_description TEXT,             -- AI 生成的图片描述
    created_at TEXT NOT NULL,
    FOREIGN KEY (message_id) REFERENCES dispatcher_messages(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_chat_images_workspace
ON chat_images(workspace_id, created_at);

CREATE INDEX IF NOT EXISTS idx_chat_images_message
ON chat_images(message_id);
```

---

### 3.4 文件系统目录结构

```
~/.jkcodingagent/
├── projects.json                    # 现有：项目列表
├── projects/<pid>/tasks.json        # 现有：任务列表
├── chat-images/                     # 新增：按会话标题隔离的图片目录
│   └── {session-title-slug}/        # 会话标题的 URL-safe slug
│       ├── {image-id}.png           # 用户粘贴 / 工具生成的图片
│       ├── {image-id}.jpg
│       └── ...
└── dispatcher.db                    # 现有：SQLite 数据库
```

**目录命名规则**：
- `session-title-slug`：会话标题的 kebab-case 形式（如 `"架构设计讨论"` → `"jia-gou-she-ji-tao-lun"`）
- 若标题为空，使用 `"untitled-{session-id-short}"`
- 若存在同名 slug，追加 `-{index}`

**文件命名规则**：
- `{uuid}.{ext}`：其中 `uuid` 为 `imageId`，`ext` 根据 mimeType 确定

---

## 四、关键代码改造点

### 4.1 前端改造

#### 4.1.1 新增工具函数：`segmentsToMarkdown`（`src/utils/segments.ts`，新建）

```typescript
import type { AnyContentSegment, TextSegment, ImageSegment } from "../types";

/**
 * 将有序片段数组组装为 Markdown 字符串
 * 用于：渲染消息、发送给 LLM
 */
export function segmentsToMarkdown(segments: AnyContentSegment[]): string {
  return segments
    .map((seg) => {
      if (seg.type === "text") {
        return (seg as TextSegment).text;
      }
      if (seg.type === "image") {
        const img = seg as ImageSegment;
        return `![${img.alt || "image"}](${img.path})`;
      }
      return "";
    })
    .join("\n");
}

/**
 * 从 Markdown 字符串解析为片段数组（用于旧数据迁移，可选）
 */
export function markdownToSegments(markdown: string): AnyContentSegment[] {
  // 解析 Markdown 中的图片语法，拆分为 TextSegment + ImageSegment
  const segments: AnyContentSegment[] = [];
  const regex = /([^]*)!\[([^]*)\]\(([^)]+)\)/g;
  let lastIndex = 0;
  let match;

  while ((match = regex.exec(markdown)) !== null) {
    const [, beforeText, alt, path] = match;
    if (beforeText) {
      segments.push({ id: crypto.randomUUID(), type: "text", text: beforeText.trim() });
    }
    segments.push({
      id: crypto.randomUUID(),
      type: "image",
      imageId: crypto.randomUUID(),
      path,
      alt: alt || undefined,
      source: "user_paste",
    });
    lastIndex = regex.lastIndex;
  }

  // 剩余文本
  const remaining = markdown.slice(lastIndex).trim();
  if (remaining) {
    segments.push({ id: crypto.randomUUID(), type: "text", text: remaining });
  }

  return segments;
}
```

#### 4.1.2 粘贴图片逻辑改造（`src/components/DispatcherChat.tsx`）

**当前逻辑**：
```typescript
const handlePaste = (e: React.ClipboardEvent) => {
  // base64 data URL → setAttachedImages
};

// 发送时
const imageMarkdown = images.map((img) => `![image](${img})\n`).join("");
content = imageMarkdown + content;
```

**改造后逻辑**：
```typescript
const handlePaste = useCallback(async (e: React.ClipboardEvent) => {
  const items = e.clipboardData?.items;
  if (!items) return;

  for (let i = 0; i < items.length; i++) {
    if (items[i].type.indexOf("image") !== -1) {
      const blob = items[i].getAsFile();
      if (!blob) continue;

      // 1. 上传图片到 Tauri，获取 asset:// 路径
      const reader = new FileReader();
      reader.onload = async (event) => {
        const base64 = event.target?.result as string;
        const result = await invoke<{ path: string; imageId: string }>(
          "save_chat_image",
          {
            sessionId,
            sessionTitle: "当前会话标题",  // 从 state 中获取
            imageDataBase64: base64.split(",")[1],  // 去掉 data:image/png;base64, 前缀
            mimeType: blob.type,
          }
        );

        // 2. 插入到 segments（光标位置或末尾）
        setSegments((prev) => [
          ...prev,
          {
            id: crypto.randomUUID(),
            type: "image",
            imageId: result.imageId,
            path: result.path,
            source: "user_paste",
            mimeType: blob.type,
          } as ImageSegment,
        ]);
      };
      reader.readAsDataURL(blob);
    }
  }
}, [sessionId, sessionTitle]);
```

#### 4.1.3 发送消息逻辑改造（`src/components/DispatcherChat.tsx`）

**当前**：
```typescript
const sendUserMessage = async (text: string, images: string[], targetSessionId: string) => {
  let content = text;
  if (images.length > 0) {
    const imageMarkdown = images.map((img) => `![image](${img})\n`).join("");
    content = imageMarkdown + content;
  }
  // send with content...
};
```

**改造后**：
```typescript
const sendUserMessage = async (
  segments: AnyContentSegment[],
  targetSessionId: string
) => {
  // 1. 调用后端命令（直接传递 segments）
  await invoke("dispatcher_send_message", {
    workspaceId: targetSessionId,
    segments,              // JSON 数组，后端序列化为 segments_json
    // content 已移除
  });
};

// 发送按钮
const handleSend = useCallback(async () => {
  if (segments.length === 0 || isLoading) return;

  try {
    await sendUserMessage(segments, sessionId);
    setSegments([]);  // 清空输入区
  } catch (err) {
    console.error("发送消息失败:", err);
  }
}, [segments, isLoading, sessionId, sendUserMessage]);
```

#### 4.1.4 渲染层改造（`src/components/DispatcherChat.tsx`）

```typescript
// 渲染消息时，组装 Markdown
const displayItems = useMemo(() => {
  return messages.map((msg) => ({
    ...msg,
    // 动态组装 Markdown 用于渲染
    content: segmentsToMarkdown(msg.segments),  // 新增字段
  }));
}, [messages]);
```

> **注意**：`MarkdownRenderer` 本身不需要改造，仍然接收 `content: string`。

---

### 4.2 Tauri (Rust) 改造

#### 4.2.1 新增 Tauri 命令：`save_chat_image`

**文件**：`src-tauri/src/chat_images.rs`（新建）

```rust
use std::path::PathBuf;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// 保存聊天图片到文件系统
#[tauri::command]
pub async fn save_chat_image(
    app_handle: tauri::AppHandle,
    session_id: String,
    session_title: String,
    image_data_base64: String,
    mime_type: String,
) -> Result<SaveImageResult, String> {
    // 1. 生成图片 ID
    let image_id = Uuid::new_v4().to_string();

    // 2. 构建目录路径：~/.jkcodingagent/chat-images/{session-title-slug}/
    let app_dir = app_data_dir()?;
    let slug = slugify(&session_title);
    let images_dir = app_dir.join("chat-images").join(slug);
    std::fs::create_dir_all(&images_dir).map_err(|e| e.to_string())?;

    // 3. 确定文件扩展名
    let ext = match mime_type.as_str() {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/webp" => "webp",
        _ => "png",
    };

    // 4. 解码 base64 并保存
    let file_path = images_dir.join(format!("{}.{}", image_id, ext));
    let image_bytes = base64::decode(&image_data_base64).map_err(|e| e.to_string())?;
    std::fs::write(&file_path, &image_bytes).map_err(|e| e.to_string())?;

    // 5. 返回 asset:// 路径
    Ok(SaveImageResult {
        image_id: image_id.clone(),
        path: format!("asset://{}", file_path.to_string_lossy()),
    })
}

#[derive(Serialize)]
pub struct SaveImageResult {
    pub image_id: String,
    pub path: String,
}

fn slugify(s: &str) -> String {
    s.to_lowercase()
        .replace(|c: char| !c.is_alphanumeric() && c != ' ', "-")
        .replace(' ', "-")
        .replace("--", "-")
}
```

#### 4.2.2 改造 `DispatcherDb`：读写 `segments_json`

**文件**：`src-tauri/src/agent/db.rs`

```rust
impl DispatcherDb {
    /// 保存消息（改造后：接收 segments，序列化为 JSON）
    pub fn save_message(
        &self,
        params: &SaveMessageParams,
    ) -> Result<DispatcherMessageRecord> {
        let conn = self.connect()?;
        let now = Utc::now().to_rfc3339();

        // 1. 序列化 segments 为 JSON
        let segments_json = serde_json::to_string(&params.segments)
            .context("serialize segments to JSON")?;

        // 2. 写入 dispatcher_messages（无 content 字段）
        conn.execute(
            "INSERT INTO dispatcher_messages (
                id, workspace_id, role, segments_json, thinking_content,
                thinking_elapsed_ms, context_payload, tool_call_id, tool_name,
                tool_result_mode, tool_artifacts_json, tool_calls_json,
                usage_stats_json, visible, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            params![
                &params.id,
                &params.workspace_id,
                &params.role,
                &segments_json,          // ← 替换原来的 content
                &params.thinking_content,
                &params.thinking_elapsed_ms,
                &params.context_payload,
                &params.tool_call_id,
                &params.tool_name,
                &params.tool_result_mode,
                Option::<String>::None,    // tool_artifacts_json
                &params.tool_calls_json,
                Option::<String>::None,    // usage_stats_json
                if params.visible { 1 } else { 0 },
                &now
            ],
        )
        .context("insert dispatcher message")?;

        // 3. 如果有图片片段，写入 chat_images 表
        for (idx, seg) in params.segments.iter().enumerate() {
            if let ContentSegment::Image(img) = seg {
                self.save_chat_image_meta(
                    &params.workspace_id,
                    &params.id,
                    idx,
                    img,
                )?;
            }
        }

        // 4. 返回记录
        Ok(DispatcherMessageRecord {
            id: params.id.clone(),
            workspace_id: params.workspace_id.clone(),
            role: params.role.clone(),
            segments_json,               // ← 返回序列化后的 JSON
            // ...
        })
    }

    /// 读取消息（反序列化 segments_json）
    pub fn load_message(&self, message_id: &str) -> Result<DispatcherMessageRecord> {
        let conn = self.connect()?;
        let row = conn.query_row(
            "SELECT id, workspace_id, role, segments_json, ... FROM dispatcher_messages WHERE id = ?1",
            params![message_id],
            |row| {
                let segments_json: String = row.get(3)?;
                let segments: Vec<ContentSegment> = serde_json::from_str(&segments_json)
                    .map_err(|e| rusqlite::Error::FromSql(rusqlite::types::FromSqlError::Other(Box::new(e))))?;

                Ok(DispatcherMessageRecord {
                    id: row.get(0)?,
                    workspace_id: row.get(1)?,
                    role: row.get(2)?,
                    segments,
                    // ...
                })
            },
        )?;

        Ok(row)
    }
}
```

#### 4.2.3 图片生成工具接口（TODO）

**文件**：`src-tauri/src/tools/image_generator.rs`（新建，留空实现）

```rust
//! 图片生成工具
//! TODO: 暂不实现生成逻辑，仅定义接口

use serde::{Deserialize, Serialize};

/// 图片生成工具入参
#[derive(Debug, Clone, Deserialize)]
pub struct ImageGenerationInput {
    pub prompt: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub style: Option<String>,
    pub negative_prompt: Option<String>,
    pub model: Option<String>,
    pub seed: Option<u64>,
}

/// 图片生成工具出参
#[derive(Debug, Clone, Serialize)]
pub struct ImageGenerationOutput {
    pub image_id: String,
    pub path: String,       // asset://... 本地路径
    pub width: u32,
    pub height: u32,
    pub mime_type: String,
    pub generation_prompt: String,
    pub generation_params: serde_json::Value,
    pub created_at: String,
}

/// TODO: 实现图片生成
/// 调用外部 API（如 Stable Diffusion / DALL-E / 本地模型）生成图片，
/// 保存到 ~/.jkcodingagent/chat-images/{session-title}/ 目录，
/// 返回 ImageGenerationOutput。
pub async fn generate_image(
    _input: ImageGenerationInput,
    _session_id: String,
    _session_title: String,
) -> anyhow::Result<ImageGenerationOutput> {
    todo!("图片生成工具待实现")
}
```

---

### 4.3 LanceDB 预留扩展（暂不实现）

#### 预留：向量表结构（`src-tauri/src/vector_store.rs` 或扩展 `db.rs`）

```rust
// 预留：多模态向量检索表
// TODO: 暂不实现，但数据结构需与 chat_images 表对齐

/*
use lancedb::{connect, Database, Table};
use arrow_array::{FixedSizeListArray, RecordBatch, StringArray, Float32Array};
use arrow_schema::{DataType, Field, Schema};

pub struct MultimodalVectorStore {
    db: Database,
}

impl MultimodalVectorStore {
    pub async fn init(db_path: &str) -> anyhow::Result<Self> {
        let db = connect(db_path).execute().await?;
        Ok(Self { db })
    }

    pub async fn create_chat_vectors_table(&self) -> anyhow::Result<Table> {
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Utf8, false),               // 对应 chat_images.id
            Field::new("segment_id", DataType::Utf8, false),       // 对应 message_segments.id
            Field::new("message_id", DataType::Utf8, false),
            Field::new("workspace_id", DataType::Utf8, false),
            Field::new("content_type", DataType::Utf8, false),     // "text" | "image"
            Field::new("text_content", DataType::Utf8, true),        // 文本内容或图片描述
            Field::new("image_path", DataType::Utf8, true),        // 图片本地路径
            // 向量字段（假设 768 维）
            Field::new("vector", DataType::FixedSizeList(
                Arc::new(Field::new("item", DataType::Float32, false)),
                768,
            ), false),
            Field::new("created_at", DataType::Int64, false),
        ]));

        let table = self.db
            .create_table("chat_vectors", schema.into())
            .execute()
            .await?;

        Ok(table)
    }

    // TODO: 实现写入和检索
}
*/
```

---

## 五、执行步骤清单

### Phase 1：数据模型与表结构（预计 1-2 天）

| # | 步骤 | 文件 | 说明 |
|---|------|------|------|
| 1.1 | 新增 TS 类型 | `src/types.ts` | `ContentSegment`, `TextSegment`, `ImageSegment`, `FileSegment`, `ImageGenerationInput/Output` |
| 1.2 | 新增 Rust 结构体 | `src-tauri/src/agent/db.rs` | `ContentSegment` 枚举、`ImageMeta`、`FileMeta` |
| 1.3 | **改造表** | `src-tauri/src/agent/db.rs` | `dispatcher_messages`：移除 `content`，新增 `segments_json` |
| 1.4 | 创建表 | `src-tauri/src/agent/db.rs` | `chat_images` 表 |
| 1.5 | 注册迁移 | `src-tauri/src/agent/db.rs` | `create_tables` 中修改 `dispatcher_messages` 表结构 |

### Phase 2：文件系统与 Tauri 命令（预计 1-2 天）

| # | 步骤 | 文件 | 说明 |
|---|------|------|------|
| 2.1 | 新建模块 | `src-tauri/src/chat_images.rs` | `save_chat_image` 命令 |
| 2.2 | 注册命令 | `src-tauri/src/lib.rs` | 在 `invoke_handler!` 中注册 `save_chat_image` |
| 2.3 | 验证路径 | `src-tauri/src/chat_images.rs` | 确保路径安全（防目录遍历） |
| 2.4 | 实现 slugify | `src-tauri/src/chat_images.rs` | 会话标题转目录名 |

### Phase 3：前端粘贴与发送逻辑（预计 1-2 天）

| # | 步骤 | 文件 | 说明 |
|---|------|------|------|
| 3.1 | 新增工具函数 | `src/utils/segments.ts`（新建） | `segmentsToMarkdown()`、`markdownToSegments()` |
| 3.2 | 改造粘贴 | `src/components/DispatcherChat.tsx` | `handlePaste` 调用 `save_chat_image` |
| 3.3 | 改造发送 | `src/components/DispatcherChat.tsx` | `sendUserMessage` 接收 `segments` 参数 |
| 3.4 | 改造渲染 | `src/components/DispatcherChat.tsx` | 渲染消息时用 `segmentsToMarkdown` 组装 |

### Phase 4：Rust DB 层读写改造（预计 2-3 天）

| # | 步骤 | 文件 | 说明 |
|---|------|------|------|
| 4.1 | **改造保存** | `src-tauri/src/agent/db.rs` | `save_message` 接收 `segments`，序列化为 `segments_json` |
| 4.2 | **改造读取** | `src-tauri/src/agent/db.rs` | `load_message` 反序列化 `segments_json` |
| 4.3 | 改造发送命令 | `src-tauri/src/agent/commands.rs` | `dispatcher_send_message` 接收 `segments` |
| 4.4 | 组装 Markdown | `src-tauri/src/agent/commands.rs` | 调用 LLM 前用 `segments_to_markdown` 组装 |

### Phase 5：图片生成工具预留（TODO，不实现）

| # | 步骤 | 文件 | 说明 |
|---|------|------|------|
| 5.1 | 新建接口文件 | `src-tauri/src/tools/image_generator.rs` | 定义 `ImageGenerationInput` / `ImageGenerationOutput` |
| 5.2 | 新建工具注册 | `src-tauri/src/tools/mod.rs` | 工具模块入口 |
| 5.3 | 标记 TODO | `src-tauri/src/tools/image_generator.rs` | `todo!()` 占位 |

### Phase 6：LanceDB 预留扩展（TODO，不实现）

| # | 步骤 | 文件 | 说明 |
|---|------|------|------|
| 6.1 | 新建模块 | `src-tauri/src/vector_store.rs` | 注释掉的向量表结构 |
| 6.2 | 预留字段 | `chat_images` 表 | `vector_embedding_json`, `text_description` |

---

## 六、关键注意事项

### 6.1 不向后兼容

- `dispatcher_messages` 表的 `content` 字段已移除
- 所有历史数据需要**重建或清空**
- 如果必须保留旧数据，可以在迁移脚本中解析旧 `content` 字段，转换为 `segments_json`

### 6.2 路径安全

- Tauri 命令中必须验证目标路径在 `~/.jkcodingagent/chat-images/` 范围内
- 禁止用户通过 `session_title` 注入路径（如 `../../etc/passwd`）
- 使用 `PathBuf::join()` 前对 `session_title` 进行 sanitize

### 6.3 性能考虑

- 图片保存到文件系统，不在 SQLite 中存储 blob
- `segments_json` 存储在单字段中，查询时直接反序列化
- 渲染时动态组装 Markdown（O(n) 字符串拼接，性能可忽略）
- `chat_images` 表按需查询（仅在需要结构化数据时 JOIN）

### 6.4 跨平台

- macOS: `~/Library/Application Support/com.jkcodingagent.app/chat-images/`
- Windows: `%APPDATA%\com.jkcodingagent.app\chat-images\`
- Linux: `~/.local/share/com.jkcodingagent.app/chat-images/`

> 当前代码使用 `app_data_dir()` 获取目录，已处理跨平台。

---

## 七、验证清单

改造完成后需验证：

- [ ] `dispatcher_messages` 表已移除 `content` 字段，新增 `segments_json` 字段
- [ ] 粘贴图片后，图片保存到 `~/.jkcodingagent/chat-images/{slug}/`
- [ ] 发送消息后，`segments_json` 存储正确（JSON 数组，包含文本+图片）
- [ ] 刷新页面后，消息中的图片正常显示（通过 `asset://`）
- [ ] 图片在 Markdown 中的位置与粘贴时一致
- [ ] LLM 调用时，`segments_to_markdown` 组装正确的 Markdown
- [ ] `chat_images` 表正确记录图片元数据

---

## 附录：相关文件索引

| 文件 | 职责 | 改造范围 |
|------|------|---------|
| `src/types.ts` | TypeScript 类型定义 | 新增 segments 类型 |
| `src/utils/segments.ts`（新建） | segments 与 Markdown 互转 | 新建 |
| `src/components/DispatcherChat.tsx` | 消息发送/接收 UI | 改造粘贴、发送、渲染逻辑 |
| `src/components/markdown/MarkdownRenderer.tsx` | Markdown 渲染 | 无需改造 |
| `src-tauri/src/agent/db.rs` | SQLite 数据库操作 | **改造表结构、读写逻辑** |
| `src-tauri/src/agent/commands.rs` | Tauri 命令处理 | 改造发送命令 |
| `src-tauri/src/chat_images.rs`（新建） | 图片文件系统操作 | 新建 |
| `src-tauri/src/tools/image_generator.rs`（新建） | 图片生成工具（TODO） | 新建（接口定义） |
| `src-tauri/src/vector_store.rs`（新建） | 向量检索（TODO） | 新建（预留） |
| `src-tauri/src/lib.rs` | Tauri 入口 | 注册新命令 |
