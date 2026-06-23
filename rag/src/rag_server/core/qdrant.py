"""Qdrant 客户端占位工厂。

骨架阶段不真正连接 Qdrant，仅保留接口形态，方便后续接入真实检索逻辑。
真实实现需注意：
  - 客户端应在 RagSettings.reload() 之后被重建
  - collection 名应拼接 collection_prefix 以做多项目隔离
"""

from __future__ import annotations

from typing import Optional

from ..config import QdrantConfig


class QdrantClientHolder:
    """Qdrant 客户端的惰性持有者。

    骨架实现：`get()` 永远返回 None；真实版本应缓存 qdrant_client.QdrantClient
    实例，并在配置变更时调用 `reset()` 让下一次访问重建连接。
    """

    def __init__(self, config: QdrantConfig) -> None:
        self._config = config
        self._client: Optional[object] = None  # 真实类型: qdrant_client.QdrantClient

    @property
    def config(self) -> QdrantConfig:
        return self._config

    def update_config(self, config: QdrantConfig) -> None:
        """配置变更后失效现有连接。"""
        self._config = config
        self._client = None

    def get(self) -> Optional[object]:
        """获取客户端实例（骨架阶段返回 None）。"""
        # TODO: 接入真实 Qdrant：
        #   from qdrant_client import QdrantClient
        #   if self._client is None:
        #       self._client = QdrantClient(
        #           url=self._config.url,
        #           api_key=self._config.api_key or None,
        #           timeout=self._config.timeout,
        #       )
        #   return self._client
        return self._client

    def collection_name(self, scope: str) -> str:
        """根据作用域（如 workspace_id）拼接完整 collection 名。"""
        safe_scope = scope.strip().replace("/", "_").replace(":", "_")
        return f"{self._config.collection_prefix}{safe_scope}"
