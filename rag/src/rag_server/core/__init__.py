"""core 子包：分片、导入与 Qdrant 入库。"""

from .qdrant import collection_name, index_documents

__all__ = ["collection_name", "index_documents"]
