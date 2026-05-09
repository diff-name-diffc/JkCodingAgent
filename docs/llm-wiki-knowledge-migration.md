# LLM Wiki 知识库迁移说明

## 已迁移能力

- 首页侧栏新增「知识库」页签，与「项目 / 分析」并列，不绑定当前代码项目。
- 知识库按集合管理，每个集合拥有独立的源文件、Wiki 页面、媒体文件和向量索引。
- 支持导入 Markdown/TXT/代码文本、PDF、DOCX、PPTX、XLSX/XLS/ODS、ODT/ODP 与常见图片文件。
- PDF/Office 图片会保存到集合的 `wiki/media/<sourceSlug>/`，页面预览通过 Tauri asset 协议读取本地图片。
- 配置多模态模型后，导入链路会为图片生成 caption，并将 caption 注入源材料上下文供 Wiki 生成与后续向量化使用。
- 文本模型使用 OpenAI-compatible `chat/completions`，用于源文件解析、Wiki 页面生成和已有页面合并。
- Embedding 模型使用 OpenAI-compatible `embeddings`，用于重建 LanceDB chunk 级向量索引和混合检索。
- 检索由后端统一实现：token 命中 + LanceDB vector 命中 + RRF 融合，UI 和 Agent 工具共用同一路径。
- 知识图谱按 wikilink 与 `sources` 重叠构建，前端使用原生 SVG 渲染。
- Wiki 页面支持列表、阅读预览和 Monaco Markdown 编辑，不迁移 Milkdown。
- 新增 Agent 工具 `search_knowledge_base` 和 `read_knowledge_page`，仅 Default 模式开放，Plan 模式不开放。

## 数据目录

所有知识库数据落在用户目录：

```text
~/.jkcodingagent/knowledge/
├── collections.json
├── settings.json
├── ingest-jobs.json
└── collections/
    └── <collectionId>/
        ├── raw/
        │   ├── sources/
        │   └── assets/
        ├── wiki/
        │   ├── overview.md
        │   ├── entities/
        │   ├── concepts/
        │   ├── sources/
        │   ├── queries/
        │   ├── comparisons/
        │   ├── synthesis/
        │   └── media/
        └── .llm-wiki/
            ├── ingest-cache.json
            └── lancedb/
                └── wiki_chunks_v2
```

导入命令只接受外部绝对路径；导入后文件会复制到 `raw/sources/`。后续页面读写、删除、检索都使用 `collectionId + relativePath`，避免暴露任意路径操作面。

## Tauri 命令

集合：

- `knowledge_list_collections`
- `knowledge_create_collection`
- `knowledge_update_collection`
- `knowledge_delete_collection`

设置：

- `knowledge_get_settings`
- `knowledge_save_settings`
- `knowledge_test_model`

导入：

- `knowledge_import_sources`
- `knowledge_get_ingest_jobs`
- `knowledge_cancel_ingest`
- `knowledge_retry_ingest`

页面：

- `knowledge_list_pages`
- `knowledge_read_page`
- `knowledge_write_page`
- `knowledge_delete_page`

检索与图谱：

- `knowledge_search`
- `knowledge_reindex_collection`
- `knowledge_vector_stats`
- `knowledge_build_graph`

重型文件 I/O、PDFium、Office 解析和图片读取均在 blocking pool 中执行。PDFium 调用带全局锁和 panic guard，损坏文件会返回错误，不会让 Tauri 进程崩掉。

## Agent 工具

`search_knowledge_base`

- 参数：`query: string`，可选 `collection_ids: string[]`，可选 `limit: number`。
- 返回：集合名、页面标题、页面类型、snippet、score、页面相对路径。
- 要求：已配置 embedding 模型并完成索引。

`read_knowledge_page`

- 参数：`collection_id: string`，`relative_path: string`，可选 `max_chars: number`。
- 返回：页面内容，默认截断保护上下文。

两个工具加入 Default 模式 allowlist；Plan 模式不开放，避免计划阶段隐式检索用户知识库。

## 模型配置

知识库配置独立存放于 `~/.jkcodingagent/knowledge/settings.json`：

- `textModel`：源文件解析、Wiki 页面生成、页面合并。
- `visionModel`：图片 caption。留空时保留图片引用，不生成 caption。
- `embeddingModel`：LanceDB 向量索引与混合检索。

每个模型配置包含：

```json
{
  "url": "https://api.example.com/v1",
  "apiKey": "sk-...",
  "model": "model-name"
}
```

`url` 可填 base URL，也可填完整 endpoint：

- chat：`/v1/chat/completions` 会被规范化为 `/v1` base。
- embedding：base URL 会自动拼为 `/v1/embeddings`。

## 导入与合并行为

- 多次导入同一源文件时，用 SHA-256 内容 hash 判断是否变化。
- hash 未变化且历史输出页面仍存在时，导入任务标记为 `skipped`。
- hash 变化时重新解析源文件，写入页面前执行 Markdown 清理和 frontmatter 补全。
- 目标页面已存在时，先把新内容交给文本模型合并；合并结果缺少 frontmatter 会被拒绝写入。
- PDF 和 Office 图片落到 `wiki/media/<sourceSlug>/`，Markdown 中使用 `media/<sourceSlug>/img-N.*` 引用。
- 当前不做原生以图搜图；图片通过 caption 文本进入 Wiki 生成和 embedding 检索链路。

## 未迁移能力

以下 llm-wiki 周边能力未迁移，原因是它们不属于本次“知识库核心”边界：

- deep research
- review
- lint
- chat
- clip server
- update check
- web search
- Milkdown 编辑器
- Sigma/Louvain 图谱渲染

## 手工验证流程

1. 启动应用，进入首页「知识库」页签。
2. 新建集合，确认 `~/.jkcodingagent/knowledge/collections/<collectionId>/` 下出现 `raw/`、`wiki/`、`.llm-wiki/lancedb/`。
3. 在设置中配置文本模型和 embedding 模型；如需图片 caption，配置多模态模型。
4. 导入 Markdown、PDF、DOCX、PPTX、XLSX 和图片文件，确认导入任务为 `done`。
5. 打开 Wiki 页面，确认页面可编辑、Markdown 可预览，PDF/Office 图片可显示。
6. 再次导入未变化文件，确认任务为 `skipped`。
7. 修改源文件后二次导入，确认页面更新且不会重复造页。
8. 点击「重建索引」，确认 chunk 数和维度有值。
9. 在「搜索」页签输入查询，确认返回标题、snippet、score 和相对路径。
10. 在 Agent Default 模式调用 `search_knowledge_base`，再用 `read_knowledge_page` 读取命中页面。
11. 打开「图谱」页签，确认 wikilink/source-overlap 能生成边并可点击跳转。

## 验证命令

```bash
pnpm lint
pnpm test
pnpm build
cd src-tauri && cargo test
```
