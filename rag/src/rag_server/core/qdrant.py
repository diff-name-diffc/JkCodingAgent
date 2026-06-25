"""Qdrant 混合向量入库。"""

from __future__ import annotations

import re

from langchain_core.documents import Document
from langchain_openai import OpenAIEmbeddings
from langchain_qdrant import FastEmbedSparse, QdrantVectorStore, RetrievalMode
from qdrant_client import QdrantClient, models
from qdrant_client.http.exceptions import UnexpectedResponse

from ..config import RagSettings


def collection_name(settings: RagSettings, project_id: str) -> str:
    """按项目隔离 collection。"""
    safe_scope = re.sub(r"[^a-zA-Z0-9_.-]+", "_", project_id.strip()).strip("_")
    if not safe_scope:
        raise ValueError("projectId 不能为空")
    return f"{settings.qdrant.collection_prefix}{safe_scope}"


def index_documents(settings: RagSettings, project_id: str, source: str, docs: list[Document]) -> int:
    """删除同源旧 points，并写入新的 child documents。"""
    if not docs:
        return 0
    client = _client(settings)
    name = collection_name(settings, project_id)
    _ensure_collection(client, name, settings)
    _delete_existing_source(client, name, project_id, source)

    store = QdrantVectorStore(
        client=client,
        collection_name=name,
        embedding=_dense_embeddings(settings),
        sparse_embedding=_sparse_embeddings(settings),
        retrieval_mode=RetrievalMode.HYBRID,
        vector_name=settings.qdrant.dense_vector_name,
        sparse_vector_name=settings.qdrant.sparse_vector_name,
    )
    store.add_documents(docs, ids=[str(doc.metadata["childId"]) for doc in docs])
    return len(docs)


def _client(settings: RagSettings) -> QdrantClient:
    return QdrantClient(
        url=settings.qdrant.url,
        api_key=settings.qdrant.api_key or None,
        timeout=settings.qdrant.timeout,
    )


def _dense_embeddings(settings: RagSettings) -> OpenAIEmbeddings:
    if settings.embedding.provider != "openai_compatible":
        raise ValueError(f"暂不支持的 Embedding provider：{settings.embedding.provider}")
    if not settings.embedding.base_url.strip():
        raise ValueError("Embedding 接口地址不能为空")
    if not settings.embedding.model.strip():
        raise ValueError("Embedding 模型名不能为空")
    return OpenAIEmbeddings(
        model=settings.embedding.model,
        api_key=settings.embedding.api_key or None,
        base_url=_embedding_base_url(settings.embedding.base_url),
    )


def _sparse_embeddings(settings: RagSettings) -> FastEmbedSparse:
    if settings.sparse_embedding.provider != "fastembed":
        raise ValueError(f"暂不支持的稀疏向量 provider：{settings.sparse_embedding.provider}")
    return FastEmbedSparse(model_name=settings.sparse_embedding.model)


def _embedding_base_url(raw: str) -> str:
    base = raw.strip().rstrip("/")
    return base.removesuffix("/embeddings")


def _ensure_collection(client: QdrantClient, name: str, settings: RagSettings) -> None:
    if _collection_exists(client, name):
        _validate_collection(client, name, settings)
        return
    client.create_collection(
        collection_name=name,
        vectors_config={
            settings.qdrant.dense_vector_name: models.VectorParams(
                size=settings.embedding.dimension,
                distance=models.Distance.COSINE,
            )
        },
        sparse_vectors_config={
            settings.qdrant.sparse_vector_name: _sparse_vector_params(),
        },
    )


def _collection_exists(client: QdrantClient, name: str) -> bool:
    try:
        return bool(client.collection_exists(collection_name=name))
    except (AttributeError, UnexpectedResponse):
        try:
            client.get_collection(collection_name=name)
            return True
        except UnexpectedResponse:
            return False


def _validate_collection(client: QdrantClient, name: str, settings: RagSettings) -> None:
    info = client.get_collection(collection_name=name)
    params = info.config.params
    vectors = params.vectors
    dense = vectors.get(settings.qdrant.dense_vector_name) if isinstance(vectors, dict) else None
    if dense is None:
        raise ValueError(f"Qdrant collection `{name}` 缺少稠密向量 `{settings.qdrant.dense_vector_name}`")
    if int(dense.size) != int(settings.embedding.dimension):
        raise ValueError(
            f"Qdrant collection `{name}` 稠密向量维度不匹配："
            f"配置 {settings.embedding.dimension}，实际 {dense.size}"
        )
    sparse_vectors = getattr(params, "sparse_vectors", None) or {}
    if settings.qdrant.sparse_vector_name not in sparse_vectors:
        raise ValueError(f"Qdrant collection `{name}` 缺少稀疏向量 `{settings.qdrant.sparse_vector_name}`")


def _sparse_vector_params() -> models.SparseVectorParams:
    try:
        return models.SparseVectorParams(
            index=models.SparseIndexParams(on_disk=False),
            modifier=models.Modifier.IDF,
        )
    except TypeError:
        return models.SparseVectorParams(index=models.SparseIndexParams(on_disk=False))


def _delete_existing_source(client: QdrantClient, name: str, project_id: str, source: str) -> None:
    selector = models.FilterSelector(
        filter=models.Filter(
            must=[
                models.FieldCondition(key="metadata.projectId", match=models.MatchValue(value=project_id)),
                models.FieldCondition(key="metadata.source", match=models.MatchValue(value=source)),
            ]
        )
    )
    client.delete(collection_name=name, points_selector=selector, wait=True)
