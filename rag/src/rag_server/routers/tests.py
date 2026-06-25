"""RAG 依赖测试路由。

这些接口故意不读取进程内配置单例：宿主每次请求都携带完整配置。
这样 sidecar 只负责执行动作，不成为第二个状态源。
"""

from __future__ import annotations

from typing import Any

import httpx
from fastapi import APIRouter, HTTPException

from ..config import RagSettings

router = APIRouter(prefix="/test", tags=["tests"])

PROBE_TEXT = "JKCodingAgent RAG embedding probe"


@router.post("/qdrant")
async def test_qdrant(payload: RagSettings) -> dict[str, Any]:
    """使用请求体中的配置测试 Qdrant HTTP 健康端点。"""
    config = payload.qdrant
    url = _qdrant_health_url(config.url)
    timeout = _positive_timeout(config.timeout, "Qdrant 超时")
    headers = _qdrant_headers(config.api_key)

    try:
        async with httpx.AsyncClient(timeout=timeout) as client:
            response = await client.get(url, headers=headers)
    except httpx.HTTPError as exc:
        raise HTTPException(status_code=502, detail=f"Qdrant 请求失败：{exc}") from exc

    if not 200 <= response.status_code < 300:
        raise HTTPException(
            status_code=502,
            detail=f"Qdrant 健康检查失败，HTTP {response.status_code}: {_body_preview(response)}",
        )

    return {
        "ok": True,
        "status": response.status_code,
        "message": "Qdrant 连接正常",
    }


@router.post("/embedding")
async def test_embedding(payload: RagSettings) -> dict[str, Any]:
    """使用请求体中的配置测试 OpenAI 兼容 Embedding 接口。"""
    config = payload.embedding
    if config.provider != "openai_compatible":
        raise HTTPException(status_code=400, detail=f"暂不支持的 Embedding provider：{config.provider}")
    if not config.model.strip():
        raise HTTPException(status_code=400, detail="Embedding 模型名不能为空")
    if config.dimension <= 0:
        raise HTTPException(status_code=400, detail="Embedding 向量维度必须大于 0")

    url = _embedding_url(config.base_url)
    headers = _embedding_headers(config.api_key)
    body = {"model": config.model.strip(), "input": PROBE_TEXT}

    try:
        async with httpx.AsyncClient(timeout=30.0) as client:
            response = await client.post(url, headers=headers, json=body)
    except httpx.HTTPError as exc:
        raise HTTPException(status_code=502, detail=f"Embedding 请求失败：{exc}") from exc

    if not 200 <= response.status_code < 300:
        raise HTTPException(
            status_code=502,
            detail=f"Embedding 测试失败，HTTP {response.status_code}: {_body_preview(response)}",
        )

    actual_dimension = _extract_embedding_dimension(response)
    if actual_dimension != config.dimension:
        raise HTTPException(
            status_code=502,
            detail=f"Embedding 维度不匹配：配置 {config.dimension}，实际 {actual_dimension}",
        )

    return {
        "ok": True,
        "dimension": actual_dimension,
        "message": f"Embedding 连接正常，维度 {actual_dimension}",
    }


def _qdrant_health_url(base_url: str) -> str:
    base = _http_base_url(base_url, "Qdrant HTTP 端点")
    return f"{base}/healthz"


def _embedding_url(base_url: str) -> str:
    base = _http_base_url(base_url, "Embedding 接口地址")
    return base if base.endswith("/embeddings") else f"{base}/embeddings"


def _http_base_url(raw: str, label: str) -> str:
    value = raw.strip().rstrip("/")
    if not value:
        raise HTTPException(status_code=400, detail=f"{label}不能为空")

    url = httpx.URL(value)
    if url.scheme not in {"http", "https"}:
        raise HTTPException(status_code=400, detail=f"{label}必须使用 http/https")
    if not url.host:
        raise HTTPException(status_code=400, detail=f"{label}缺少 host")
    return value


def _positive_timeout(value: float, label: str) -> float:
    if value <= 0:
        raise HTTPException(status_code=400, detail=f"{label}必须大于 0")
    return value


def _qdrant_headers(api_key: str) -> dict[str, str]:
    key = api_key.strip()
    return {"api-key": key} if key else {}


def _embedding_headers(api_key: str) -> dict[str, str]:
    headers = {"Content-Type": "application/json"}
    key = api_key.strip()
    if key:
        headers["Authorization"] = f"Bearer {key}"
    return headers


def _extract_embedding_dimension(response: httpx.Response) -> int:
    try:
        data = response.json()
        embedding = data["data"][0]["embedding"]
    except (ValueError, KeyError, IndexError, TypeError) as exc:
        raise HTTPException(
            status_code=502,
            detail=f"Embedding 响应结构无效：{_body_preview(response)}",
        ) from exc

    if not isinstance(embedding, list) or not embedding:
        raise HTTPException(status_code=502, detail="Embedding 响应中未返回有效向量")
    return len(embedding)


def _body_preview(response: httpx.Response) -> str:
    text = response.text.strip()
    return text[:500] if text else "<empty>"
