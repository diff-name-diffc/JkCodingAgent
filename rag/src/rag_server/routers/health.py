"""健康检查与就绪探针。

宿主（Rust）通过握手协议拿到端口后，会轮询 GET /health 确认服务就绪。
"""

from __future__ import annotations

from fastapi import APIRouter

from .. import __version__
from ..config import get_settings

router = APIRouter(tags=["health"])


@router.get("/health")
def health() -> dict:
    """返回服务状态。骨架阶段永远返回 ready。"""
    settings = get_settings()
    return {
        "status": "ready",
        "version": __version__,
        "qdrantUrl": settings.qdrant.url,
        "embeddingModel": settings.embedding.model,
    }
