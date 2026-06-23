"""core 子包：Qdrant / Embedding 等外部依赖的客户端封装。"""

from .embedding import EmbeddingClient
from .qdrant import QdrantClientHolder

__all__ = ["EmbeddingClient", "QdrantClientHolder"]
