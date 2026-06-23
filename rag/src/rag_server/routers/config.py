"""配置热更新路由。

设计背景：
  知识库配置的权威源在 Rust 宿主（~/.jkcodingagent/rag/config.json）。
  当用户在前端修改 Qdrant 连接或 embedding 模型后，宿主写盘并调用
  POST /config/reload 把新配置推送到本进程，本进程整体替换内存单例。

  骨架阶段只做内存替换与 200 返回；接入真实 Qdrant/Embedding 客户端后，
  需要在 reload 后触发客户端连接池重建。
"""

from __future__ import annotations

from fastapi import APIRouter, HTTPException
from pydantic import BaseModel

from ..config import EmbeddingConfig, QdrantConfig, RagSettings, get_settings

router = APIRouter(tags=["config"])


class ReloadPayload(BaseModel):
    """宿主推送的完整配置（字段与 RagSettings 一致）。"""

    qdrant: QdrantConfig
    embedding: EmbeddingConfig
    logLevel: str = "INFO"


@router.get("/config")
def get_config() -> dict:
    """返回当前内存中的配置（API Key 等敏感字段会被脱敏）。"""
    settings = get_settings()
    return {
        "qdrant": {
            "url": settings.qdrant.url,
            "collectionPrefix": settings.qdrant.collection_prefix,
            "timeout": settings.qdrant.timeout,
            "hasApiKey": bool(settings.qdrant.api_key),
        },
        "embedding": {
            "provider": settings.embedding.provider,
            "baseUrl": settings.embedding.base_url,
            "model": settings.embedding.model,
            "dimension": settings.embedding.dimension,
            "hasApiKey": bool(settings.embedding.api_key),
        },
        "logLevel": settings.log_level,
    }


@router.post("/config/reload")
def reload_config(payload: ReloadPayload) -> dict:
    """用一份完整新配置替换内存单例。

    返回的 `applied=true` 仅表示内存替换成功，不代表外部依赖已重连。
    """
    try:
        new_settings = RagSettings(
            qdrant=payload.qdrant,
            embedding=payload.embedding,
            log_level=payload.logLevel,
        )
        get_settings().reload(new_settings)
    except Exception as exc:  # noqa: BLE001 —— 配置构造失败需明确回传
        raise HTTPException(status_code=400, detail=f"配置无效：{exc}") from exc

    return {"applied": True}
