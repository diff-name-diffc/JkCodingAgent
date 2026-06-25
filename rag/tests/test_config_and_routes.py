"""RAG sidecar 配置解析与测试路由的基础回归。"""

from __future__ import annotations

from fastapi.testclient import TestClient
from langchain_core.documents import Document

from rag_server.config import ChunkingConfig, OcrConfig, RagSettings
from rag_server.core.chunking import build_child_documents
from rag_server.loaders import load_documents
from rag_server.main import app


def test_settings_accept_camel_case_payload() -> None:
    settings = RagSettings.model_validate(
        {
            "qdrant": {
                "url": "http://127.0.0.1:6333",
                "apiKey": "q-key",
                "collectionPrefix": "demo_",
                "timeout": 3,
            },
            "embedding": {
                "provider": "openai_compatible",
                "baseUrl": "https://example.com/v1",
                "apiKey": "e-key",
                "model": "text-embedding-3-small",
                "dimension": 1536,
            },
            "logLevel": "DEBUG",
        }
    )

    assert settings.qdrant.api_key == "q-key"
    assert settings.qdrant.collection_prefix == "demo_"
    assert settings.embedding.base_url == "https://example.com/v1"
    assert settings.embedding.api_key == "e-key"
    assert settings.log_level == "DEBUG"


def test_embedding_test_requires_base_url() -> None:
    client = TestClient(app)
    response = client.post(
        "/test/embedding",
        json={
            "qdrant": {
                "url": "http://127.0.0.1:6333",
                "apiKey": "",
                "collectionPrefix": "jk_",
                "timeout": 1,
            },
            "embedding": {
                "provider": "openai_compatible",
                "baseUrl": "",
                "apiKey": "",
                "model": "text-embedding-3-small",
                "dimension": 1536,
            },
            "logLevel": "INFO",
        },
    )

    assert response.status_code == 400
    assert response.json()["detail"] == "Embedding 接口地址不能为空"


def test_qdrant_test_rejects_non_http_url() -> None:
    client = TestClient(app)
    response = client.post(
        "/test/qdrant",
        json={
            "qdrant": {
                "url": "file:///tmp/qdrant",
                "apiKey": "",
                "collectionPrefix": "jk_",
                "timeout": 1,
            },
            "embedding": {
                "provider": "openai_compatible",
                "baseUrl": "https://example.com/v1",
                "apiKey": "",
                "model": "text-embedding-3-small",
                "dimension": 1536,
            },
            "logLevel": "INFO",
        },
    )

    assert response.status_code == 400
    assert response.json()["detail"] == "Qdrant HTTP 端点必须使用 http/https"


def test_text_loader_adds_project_metadata(tmp_path) -> None:
    sample = tmp_path / "notes.txt"
    sample.write_text("alpha\nbeta", encoding="utf-8")

    docs = load_documents(
        str(sample),
        project_id="proj_1",
        project_path=str(tmp_path),
        ocr=OcrConfig(enabled=False),
    )

    assert len(docs) == 1
    assert docs[0].page_content == "alpha\nbeta"
    assert docs[0].metadata["projectId"] == "proj_1"
    assert docs[0].metadata["relativePath"] == "notes.txt"


def test_parent_child_chunking_links_metadata() -> None:
    docs = [
        Document(
            page_content="alpha beta gamma delta epsilon zeta eta theta",
            metadata={"projectId": "proj_1", "source": "/tmp/doc.txt"},
        )
    ]
    children = build_child_documents(
        docs,
        ChunkingConfig(
            parentChunkSize=24,
            parentChunkOverlap=4,
            childChunkSize=12,
            childChunkOverlap=2,
            separators=[" "],
        ),
    )

    assert children
    assert all(child.metadata["parentId"] for child in children)
    assert all(child.metadata["childId"].startswith(child.metadata["parentId"]) for child in children)
    assert all(child.metadata["parentText"] for child in children)
