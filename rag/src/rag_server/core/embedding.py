"""Embedding 客户端占位。

约定走 OpenAI 兼容接口（复用宿主已有的 LLM 配置）。骨架阶段仅声明接口，
不实际发起 HTTP 请求。
"""

from __future__ import annotations

from typing import List

from ..config import EmbeddingConfig


class EmbeddingClient:
    """OpenAI 兼容 embedding 客户端的占位。"""

    def __init__(self, config: EmbeddingConfig) -> None:
        self._config = config

    @property
    def config(self) -> EmbeddingConfig:
        return self._config

    def update_config(self, config: EmbeddingConfig) -> None:
        self._config = config

    def embed(self, texts: List[str]) -> List[List[float]]:
        """把文本批量转为向量（骨架阶段抛 NotImplementedError）。"""
        # TODO: 真实实现走 httpx 调用 self._config.base_url + "/embeddings"
        raise NotImplementedError("embedding 尚未实现，等待后续迭代接入 API")

    @property
    def dimension(self) -> int:
        return self._config.dimension
