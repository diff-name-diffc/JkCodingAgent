#!/usr/bin/env python3
"""Backfill session_keywords for existing plain chat sessions.

The script mirrors the app's current keyword maintenance path:
- read the latest two visible dialogue turns for each chat session
- call the active V2 Chat summary model through an OpenAI-compatible endpoint
- apply keep/add/remove/merge actions to session_keywords

It is intentionally conservative: by default it skips sessions that already
have keywords. Use --force to rebuild them.
"""

from __future__ import annotations

import argparse
import json
import os
import sqlite3
import sys
import time
import urllib.error
import urllib.request
from dataclasses import dataclass
from pathlib import Path
from typing import Any


DEFAULT_DB_PATH = Path.home() / ".jkcodingagent" / "jkbot.sqlite3"
DEFAULT_API_BASE = "https://dashscope.aliyuncs.com/compatible-mode/v1"
KEYWORDS_QA_MAX_CHARS = 3_000
KEYWORDS_MAX = 15
ASSISTANT_MAX_CHARS = 2_000


@dataclass(frozen=True)
class SummaryConfig:
    url: str
    api_key: str
    model: str


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="为历史普通聊天会话回填 session_keywords。"
    )
    parser.add_argument("--db", default=str(DEFAULT_DB_PATH), help="SQLite 数据库路径")
    parser.add_argument("--limit", type=int, default=0, help="最多处理多少个会话，0 表示不限")
    parser.add_argument("--force", action="store_true", help="清空并重建已有关键词")
    parser.add_argument("--dry-run", action="store_true", help="只打印将处理的会话，不写库、不调模型")
    parser.add_argument("--sleep", type=float, default=0.2, help="每个会话之间暂停秒数")
    parser.add_argument("--timeout", type=float, default=120.0, help="模型请求超时时间")
    return parser.parse_args()


def connect(db_path: str) -> sqlite3.Connection:
    path = Path(db_path).expanduser()
    if not path.exists():
        raise SystemExit(f"数据库不存在：{path}")
    conn = sqlite3.connect(str(path))
    conn.row_factory = sqlite3.Row
    return conn


def active_config(configs: list[dict[str, Any]]) -> dict[str, Any] | None:
    for item in configs:
        if item.get("active"):
            return item
    return configs[0] if configs else None


def load_chat_summary_config(conn: sqlite3.Connection) -> SummaryConfig:
    row = conn.execute(
        """
        SELECT chat_agent_summary_model_configs_json
        FROM dispatcher_settings_v2
        WHERE id = 'default'
        """
    ).fetchone()
    if row is None:
        raise SystemExit("未找到 dispatcher_settings_v2.default，请先在应用中保存最新设置。")

    try:
        configs = json.loads(row["chat_agent_summary_model_configs_json"] or "[]")
    except json.JSONDecodeError as error:
        raise SystemExit(f"聊天摘要配置 JSON 无法解析：{error}") from error

    config = active_config(configs)
    if not config:
        raise SystemExit("未配置聊天摘要模型：chat_agent_summary_model_configs_json 为空。")

    model = str(config.get("model") or "").strip()
    if not model:
        raise SystemExit("聊天摘要模型名称为空，请先在最新设置中配置。")

    return SummaryConfig(
        url=str(config.get("url") or "").strip() or os.getenv("OPENAI_API_BASE", DEFAULT_API_BASE),
        api_key=str(config.get("apiKey") or config.get("api_key") or "").strip()
        or os.getenv("OPENAI_API_KEY", "")
        or os.getenv("DASHSCOPE_API_KEY", ""),
        model=model,
    )


def text_from_segments(segments_json: str) -> str:
    try:
        segments = json.loads(segments_json or "[]")
    except json.JSONDecodeError:
        return ""
    parts: list[str] = []
    for segment in segments if isinstance(segments, list) else []:
        if not isinstance(segment, dict):
            continue
        kind = segment.get("type")
        if kind == "text":
            text = str(segment.get("text") or "").strip()
            if text:
                parts.append(text)
        elif kind == "image":
            alt = str(segment.get("alt") or "图片").strip()
            parts.append(f"[图片: {alt}]")
        else:
            text = str(segment.get("text") or "").strip()
            if text:
                parts.append(text)
    return "\n".join(parts)


def list_candidate_sessions(conn: sqlite3.Connection, force: bool, limit: int) -> list[sqlite3.Row]:
    sql = """
        SELECT ds.id, COALESCE(cs.title, ds.title) AS title, ds.updated_at
        FROM dispatcher_sessions ds
        JOIN chat_sessions cs ON cs.id = ds.id
        WHERE ds.kind = 'chat'
          AND EXISTS (
            SELECT 1 FROM dispatcher_messages dm
            WHERE dm.workspace_id = ds.id
              AND dm.visible = 1
              AND dm.context_cleared = 0
              AND dm.role IN ('user', 'assistant')
          )
    """
    if not force:
        sql += """
          AND NOT EXISTS (
            SELECT 1 FROM session_keywords sk
            WHERE sk.workspace_id = ds.id
          )
        """
    sql += " ORDER BY ds.updated_at ASC"
    if limit > 0:
        sql += " LIMIT ?"
        return conn.execute(sql, (limit,)).fetchall()
    return conn.execute(sql).fetchall()


def dialogue_cutoff_rowid(conn: sqlite3.Connection, session_id: str, max_dialogues: int = 2) -> int:
    rows = conn.execute(
        """
        SELECT rowid
        FROM dispatcher_messages
        WHERE workspace_id = ?
          AND visible = 1
          AND context_cleared = 0
          AND role = 'user'
        ORDER BY created_at DESC, rowid DESC
        LIMIT ?
        """,
        (session_id, max_dialogues),
    ).fetchall()
    if not rows:
        return 0
    return min(int(row["rowid"]) for row in rows)


def recent_messages(conn: sqlite3.Connection, session_id: str) -> list[dict[str, str]]:
    cutoff = dialogue_cutoff_rowid(conn, session_id)
    rows = conn.execute(
        """
        SELECT role, segments_json
        FROM dispatcher_messages
        WHERE workspace_id = ?
          AND visible = 1
          AND context_cleared = 0
          AND rowid >= ?
        ORDER BY created_at ASC, rowid ASC
        """,
        (session_id, cutoff),
    ).fetchall()
    messages: list[dict[str, str]] = []
    for row in rows:
        content = text_from_segments(row["segments_json"])
        if content.strip():
            messages.append({"role": row["role"], "content": content})
    return messages


def existing_keywords_json(conn: sqlite3.Connection, session_id: str) -> str:
    rows = conn.execute(
        """
        SELECT keyword, weight
        FROM session_keywords
        WHERE workspace_id = ?
        ORDER BY weight DESC, keyword ASC
        """,
        (session_id,),
    ).fetchall()
    return json.dumps(
        [{"keyword": row["keyword"], "weight": row["weight"]} for row in rows],
        ensure_ascii=False,
    )


def build_qa_text(messages: list[dict[str, str]]) -> str | None:
    user = next((m for m in messages if m["role"] == "user"), None)
    assistant = next((m for m in messages if m["role"] == "assistant"), None)
    if not user or not assistant:
        return None
    assistant_text = assistant["content"]
    if len(assistant_text) > ASSISTANT_MAX_CHARS:
        assistant_text = assistant_text[:ASSISTANT_MAX_CHARS] + "..."
    return f"【用户】\n{user['content']}\n\n【助手】\n{assistant_text}\n"


def build_prompt(qa_text: str, existing_json: str) -> str:
    qa_truncated = qa_text
    if len(qa_truncated) > KEYWORDS_QA_MAX_CHARS:
        qa_truncated = qa_truncated[:KEYWORDS_QA_MAX_CHARS] + "\n...(truncated)"
    return f"""你是一个会话关键字维护助手。你的任务是根据最新一轮对话内容，维护一组关键字来描述这个会话的主题。

现有关键字（JSON 数组）：
{existing_json}

最新一轮对话（用户 + AI 助手的一问一答）：
{qa_truncated}

规则：
1. 只输出 JSON 数组，不要添加其他任何内容（不要 markdown 代码块包裹）
2. 最多 {KEYWORDS_MAX} 个关键字
3. 关键字必须简洁：2-20 字符的术语或短语
4. 保留仍然相关的关键字（"keep"）
5. 添加新出现的主题、技术、工具、概念（"add"），权重 1-10
6. 相似关键字可以合并为一个（"merge"）
7. 不再相关的旧关键字标记为删除（"remove"）
8. 代码标识符（函数名、类名、变量名）优先
9. 文件名/路径保留最后一级

输出格式（严格 JSON）：
[
  {{"action":"keep","keyword":"原关键字"}},
  {{"action":"add","keyword":"新关键字","weight":7.5}},
  {{"action":"merge","from":["旧1","旧2"],"to":"合并后","weight":6.0}},
  {{"action":"remove","keyword":"要删除的关键字"}}
]"""


def call_summary_model(config: SummaryConfig, prompt: str, timeout: float) -> str:
    if not config.api_key:
        raise RuntimeError("聊天摘要模型 API Key 为空，请在最新设置中填写或设置环境变量。")
    base = config.url.rstrip("/")
    url = f"{base}/chat/completions"
    payload = {
        "model": config.model,
        "messages": [{"role": "system", "content": prompt}],
        "stream": False,
        "temperature": 0.1,
        # 关闭思考，与后端摘要路径保持一致：reasoning 模型（如 o-deepseek-v4-flash）
        # 默认先输出 reasoning_content，正式 content 为空会触发“模型返回空内容”。
        "enable_thinking": False,
        # keywords JSON（最多 15 项）需要预算；非思考模型输出完即停，此处仅作上限。
        "max_tokens": 2048,
    }
    request = urllib.request.Request(
        url,
        data=json.dumps(payload, ensure_ascii=False).encode("utf-8"),
        headers={
            "Content-Type": "application/json",
            "Authorization": f"Bearer {config.api_key}",
        },
        method="POST",
    )
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            data = json.loads(response.read().decode("utf-8"))
    except urllib.error.HTTPError as error:
        body = error.read().decode("utf-8", errors="replace")
        raise RuntimeError(f"模型请求失败 HTTP {error.code}: {body[:500]}") from error
    content = data.get("choices", [{}])[0].get("message", {}).get("content", "")
    if not str(content).strip():
        raise RuntimeError("模型返回空内容")
    return str(content).strip()


def parse_actions(raw: str) -> list[dict[str, Any]]:
    text = raw.strip()
    if text.startswith("```"):
        lines = text.splitlines()[1:]
        text = "\n".join(line for line in lines if not line.strip().startswith("```"))
    actions = json.loads(text)
    if not isinstance(actions, list):
        raise ValueError("模型输出不是 JSON 数组")
    return [a for a in actions if isinstance(a, dict) and isinstance(a.get("action"), str)]


def apply_actions(
    conn: sqlite3.Connection,
    session_id: str,
    actions: list[dict[str, Any]],
    force: bool,
) -> int:
    timestamp = time.strftime("%Y-%m-%dT%H:%M:%S.000Z", time.gmtime())
    with conn:
        if force:
            conn.execute("DELETE FROM session_keywords WHERE workspace_id = ?", (session_id,))
        changed = 0
        for action in actions:
            kind = action["action"]
            if kind == "add":
                keyword = str(action.get("keyword") or "").strip()
                if not keyword:
                    continue
                conn.execute(
                    """
                    INSERT OR IGNORE INTO session_keywords (workspace_id, keyword, weight, created_at)
                    VALUES (?, ?, ?, ?)
                    """,
                    (session_id, keyword, float(action.get("weight") or 1.0), timestamp),
                )
                changed += 1
            elif kind == "remove":
                keyword = str(action.get("keyword") or "").strip()
                if keyword:
                    conn.execute(
                        "DELETE FROM session_keywords WHERE workspace_id = ? AND keyword = ?",
                        (session_id, keyword),
                    )
                    changed += 1
            elif kind == "merge":
                for keyword in action.get("from") or []:
                    conn.execute(
                        "DELETE FROM session_keywords WHERE workspace_id = ? AND keyword = ?",
                        (session_id, str(keyword).strip()),
                    )
                to_keyword = str(action.get("to") or "").strip()
                if to_keyword:
                    conn.execute(
                        """
                        INSERT OR REPLACE INTO session_keywords (workspace_id, keyword, weight, created_at)
                        VALUES (?, ?, ?, ?)
                        """,
                        (session_id, to_keyword, float(action.get("weight") or 1.0), timestamp),
                    )
                    changed += 1
            elif kind == "keep":
                continue
        return changed


def main() -> int:
    args = parse_args()
    conn = connect(args.db)
    config = load_chat_summary_config(conn)
    sessions = list_candidate_sessions(conn, args.force, args.limit)
    print(f"数据库：{Path(args.db).expanduser()}")
    print(f"聊天摘要模型：{config.model} @ {config.url}")
    print(f"待处理会话：{len(sessions)}")

    if args.dry_run:
        for row in sessions:
            print(f"[dry-run] {row['id']}  {row['title']}")
        return 0

    ok = 0
    skipped = 0
    failed = 0
    for index, session in enumerate(sessions, start=1):
        sid = session["id"]
        title = session["title"]
        try:
            messages = recent_messages(conn, sid)
            qa_text = build_qa_text(messages)
            if not qa_text:
                skipped += 1
                print(f"[{index}/{len(sessions)}] 跳过：{title}（缺少完整用户/助手对话）")
                continue
            prompt = build_prompt(qa_text, existing_keywords_json(conn, sid))
            raw = call_summary_model(config, prompt, args.timeout)
            actions = parse_actions(raw)
            if not actions:
                skipped += 1
                print(f"[{index}/{len(sessions)}] 跳过：{title}（模型未返回有效动作）")
                continue
            changed = apply_actions(conn, sid, actions, args.force)
            ok += 1
            print(f"[{index}/{len(sessions)}] 完成：{title}（动作 {len(actions)}，写入 {changed}）")
        except Exception as error:  # noqa: BLE001 - migration script should continue per session.
            failed += 1
            print(f"[{index}/{len(sessions)}] 失败：{title}：{error}", file=sys.stderr)
        if args.sleep > 0:
            time.sleep(args.sleep)

    print(f"完成：成功 {ok}，跳过 {skipped}，失败 {failed}")
    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main())
