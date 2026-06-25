"""知识库运行时配置。

设计原则（来自需求约定）：
  - 配置的权威存储位于 Rust 宿主（~/.jkcodingagent/rag/config.json）
  - 宿主启动 sidecar 时，通过环境变量注入初始配置
  - 配置变更时，宿主通过 HTTP POST /config/reload 热更新内存状态
  - 本进程不读写磁盘配置文件，避免双写不一致

因此这里的 RagSettings 同时承担两个入口：
  1. `from_env()` —— 进程启动时从环境变量构造
  2. `reload(payload)` —— 收到 reload 请求时整体替换内存单例
"""

from __future__ import annotations

import logging
from typing import Optional

from pydantic import BaseModel, ConfigDict, Field


class SidecarModel(BaseModel):
    """同时接受 Rust/TypeScript 的 camelCase 与 Python 内部 snake_case。"""

    model_config = ConfigDict(populate_by_name=True)


class EmbeddingConfig(SidecarModel):
    """Embedding 模型配置。"""

    provider: str = Field(default="openai_compatible", description="供应商类型")
    base_url: str = Field(
        default="",
        alias="baseUrl",
        description="OpenAI 兼容的 embedding 接口地址",
    )
    api_key: str = Field(default="", alias="apiKey", description="API Key（由宿主注入）")
    model: str = Field(default="text-embedding-3-small", description="模型名")
    dimension: int = Field(default=1536, description="向量维度，需与 Qdrant collection 一致")


class QdrantConfig(SidecarModel):
    """Qdrant 连接配置（外部独立部署）。"""

    url: str = Field(default="http://127.0.0.1:6333", description="Qdrant HTTP 端点")
    api_key: str = Field(default="", alias="apiKey", description="Qdrant API Key，可选")
    collection_prefix: str = Field(
        default="jk_",
        alias="collectionPrefix",
        description="collection 命名前缀，用于多租户/多项目隔离",
    )
    timeout: float = Field(default=10.0, description="Qdrant 请求超时（秒）")
    dense_vector_name: str = Field(default="dense", alias="denseVectorName", description="稠密向量名")
    sparse_vector_name: str = Field(default="sparse", alias="sparseVectorName", description="稀疏向量名")


class SparseEmbeddingConfig(SidecarModel):
    """稀疏向量配置。"""

    provider: str = Field(default="fastembed", description="稀疏向量供应商")
    model: str = Field(default="Qdrant/bm25", description="FastEmbed sparse 模型名")


class ChunkingConfig(SidecarModel):
    """父子分片配置。"""

    parent_chunk_size: int = Field(default=2000, alias="parentChunkSize")
    parent_chunk_overlap: int = Field(default=200, alias="parentChunkOverlap")
    child_chunk_size: int = Field(default=400, alias="childChunkSize")
    child_chunk_overlap: int = Field(default=80, alias="childChunkOverlap")
    separators: list[str] = Field(default_factory=lambda: ["\n\n", "\n", "。", "；", ". ", " ", ""])


class OcrConfig(SidecarModel):
    """OCR 配置。"""

    enabled: bool = True
    use_cuda: bool = Field(default=False, alias="useCuda")
    pdf_image_width_ratio: float = Field(default=0.6, alias="pdfImageWidthRatio")
    pdf_image_height_ratio: float = Field(default=0.6, alias="pdfImageHeightRatio")


class RagSettings(SidecarModel):
    """RAG sidecar 的完整运行时配置（内存单例）。"""

    qdrant: QdrantConfig = Field(default_factory=QdrantConfig)
    embedding: EmbeddingConfig = Field(default_factory=EmbeddingConfig)
    sparse_embedding: SparseEmbeddingConfig = Field(default_factory=SparseEmbeddingConfig, alias="sparseEmbedding")
    chunking: ChunkingConfig = Field(default_factory=ChunkingConfig)
    ocr: OcrConfig = Field(default_factory=OcrConfig)
    log_level: str = Field(default="INFO", alias="logLevel", description="日志级别")

    # ---- 两个入口 ----

    @classmethod
    def from_env(cls) -> "RagSettings":
        """从环境变量构造初始配置。

        宿主在 spawn sidecar 时通过 SidecarCommand.env() 注入以下变量：
          RAG_QDRANT_URL / RAG_QDRANT_API_KEY / RAG_QDRANT_COLLECTION_PREFIX
          RAG_EMBEDDING_BASE_URL / RAG_EMBEDDING_API_KEY / RAG_EMBEDDING_MODEL / RAG_EMBEDDING_DIMENSION
          RAG_LOG_LEVEL
        缺失项使用默认值，保证骨架可独立运行。
        """
        import os

        def _get(key: str, default: str = "") -> str:
            value = os.environ.get(key)
            return value if value is not None and value != "" else default

        dimension_raw = _get("RAG_EMBEDDING_DIMENSION", "1536")
        try:
            dimension = int(dimension_raw)
        except ValueError:
            dimension = 1536

        try:
            timeout = float(_get("RAG_QDRANT_TIMEOUT", "10.0"))
        except ValueError:
            timeout = 10.0

        return cls(
            qdrant=QdrantConfig(
                url=_get("RAG_QDRANT_URL", QdrantConfig().url),
                api_key=_get("RAG_QDRANT_API_KEY"),
                collection_prefix=_get("RAG_QDRANT_COLLECTION_PREFIX", "jk_"),
                timeout=timeout,
                dense_vector_name=_get("RAG_QDRANT_DENSE_VECTOR_NAME", "dense"),
                sparse_vector_name=_get("RAG_QDRANT_SPARSE_VECTOR_NAME", "sparse"),
            ),
            embedding=EmbeddingConfig(
                provider=_get("RAG_EMBEDDING_PROVIDER", "openai_compatible"),
                base_url=_get("RAG_EMBEDDING_BASE_URL"),
                api_key=_get("RAG_EMBEDDING_API_KEY"),
                model=_get("RAG_EMBEDDING_MODEL", "text-embedding-3-small"),
                dimension=dimension,
            ),
            sparseEmbedding=SparseEmbeddingConfig(
                provider=_get("RAG_SPARSE_EMBEDDING_PROVIDER", "fastembed"),
                model=_get("RAG_SPARSE_EMBEDDING_MODEL", "Qdrant/bm25"),
            ),
            chunking=ChunkingConfig(
                parentChunkSize=_parse_int(_get("RAG_PARENT_CHUNK_SIZE", "2000"), 2000),
                parentChunkOverlap=_parse_int(_get("RAG_PARENT_CHUNK_OVERLAP", "200"), 200),
                childChunkSize=_parse_int(_get("RAG_CHILD_CHUNK_SIZE", "400"), 400),
                childChunkOverlap=_parse_int(_get("RAG_CHILD_CHUNK_OVERLAP", "80"), 80),
            ),
            ocr=OcrConfig(
                enabled=_parse_bool(_get("RAG_OCR_ENABLED", "true"), True),
                useCuda=_parse_bool(_get("RAG_OCR_USE_CUDA", "false"), False),
                pdfImageWidthRatio=_parse_float(_get("RAG_OCR_PDF_IMAGE_WIDTH_RATIO", "0.6"), 0.6),
                pdfImageHeightRatio=_parse_float(_get("RAG_OCR_PDF_IMAGE_HEIGHT_RATIO", "0.6"), 0.6),
            ),
            log_level=normalize_log_level(_get("RAG_LOG_LEVEL", "INFO")),
        )

    def reload(self, payload: "RagSettings") -> None:
        """用一份新配置整体替换当前内存状态（由 /config/reload 触发）。

        骨架阶段仅做对象替换；后续接入真实 Qdrant 客户端后，需在此重建连接池。
        """
        object.__setattr__(self, "qdrant", payload.qdrant)
        object.__setattr__(self, "embedding", payload.embedding)
        object.__setattr__(self, "sparse_embedding", payload.sparse_embedding)
        object.__setattr__(self, "chunking", payload.chunking)
        object.__setattr__(self, "ocr", payload.ocr)
        object.__setattr__(self, "log_level", normalize_log_level(payload.log_level))
        apply_log_level(self.log_level)


# 进程级单例：在 main.py 启动时由 from_env() 初始化一次，
# 后续 reload 在原对象上就地更新，避免持有旧引用的地方读到过期数据。
_settings: Optional[RagSettings] = None


def get_settings() -> RagSettings:
    global _settings
    if _settings is None:
        _settings = RagSettings.from_env()
    return _settings


def init_settings() -> RagSettings:
    """在应用启动时显式初始化单例（FastAPI lifespan 调用）。"""
    global _settings
    _settings = RagSettings.from_env()
    _settings.log_level = normalize_log_level(_settings.log_level)
    apply_log_level(_settings.log_level)
    return _settings


def normalize_log_level(value: str) -> str:
    """返回 Python logging 可识别的日志级别名。"""
    normalized = (value or "INFO").strip().upper()
    return normalized if normalized in {"DEBUG", "INFO", "WARNING", "ERROR"} else "INFO"


def _parse_int(value: str, default: int) -> int:
    try:
        return int(value)
    except ValueError:
        return default


def _parse_float(value: str, default: float) -> float:
    try:
        return float(value)
    except ValueError:
        return default


def _parse_bool(value: str, default: bool) -> bool:
    normalized = value.strip().lower()
    if normalized in {"1", "true", "yes", "on"}:
        return True
    if normalized in {"0", "false", "no", "off"}:
        return False
    return default


def apply_log_level(value: str) -> None:
    """热更新当前进程的日志等级。"""
    level_name = normalize_log_level(value)
    level = getattr(logging, level_name, logging.INFO)
    logging.getLogger().setLevel(level)
    for logger_name in ("uvicorn", "uvicorn.error", "uvicorn.access", "rag_server"):
        logging.getLogger(logger_name).setLevel(level)
    for handler in logging.getLogger().handlers:
        handler.setLevel(level)
