"""父子分片。"""

from __future__ import annotations

import hashlib

from langchain_core.documents import Document
from langchain_text_splitters import RecursiveCharacterTextSplitter

from ..config import ChunkingConfig


def build_child_documents(docs: list[Document], config: ChunkingConfig) -> list[Document]:
    """把自然粒度 Document 拆为 parent，再拆为 child。"""
    _validate_chunking(config)
    parent_splitter = RecursiveCharacterTextSplitter(
        chunk_size=config.parent_chunk_size,
        chunk_overlap=config.parent_chunk_overlap,
        separators=config.separators,
    )
    child_splitter = RecursiveCharacterTextSplitter(
        chunk_size=config.child_chunk_size,
        chunk_overlap=config.child_chunk_overlap,
        separators=config.separators,
    )

    children: list[Document] = []
    for doc_index, doc in enumerate(docs):
        parents = parent_splitter.split_text(doc.page_content)
        for parent_index, parent_text in enumerate(parents):
            parent_id = _stable_id(doc.metadata, doc_index, parent_index, parent_text)
            for child_index, child_text in enumerate(child_splitter.split_text(parent_text)):
                child_id = f"{parent_id}:{child_index}"
                metadata = {
                    **doc.metadata,
                    "parentId": parent_id,
                    "childId": child_id,
                    "parentIndex": parent_index,
                    "childIndex": child_index,
                    "parentText": parent_text,
                    "childText": child_text,
                }
                children.append(Document(page_content=child_text, metadata=metadata))
    return children


def _stable_id(metadata: dict, doc_index: int, parent_index: int, parent_text: str) -> str:
    source = str(metadata.get("source", ""))
    project_id = str(metadata.get("projectId", ""))
    digest = hashlib.sha256(
        f"{project_id}\n{source}\n{doc_index}\n{parent_index}\n{parent_text}".encode("utf-8")
    ).hexdigest()
    return digest[:32]


def _validate_chunking(config: ChunkingConfig) -> None:
    pairs = [
        ("parentChunkSize", config.parent_chunk_size),
        ("childChunkSize", config.child_chunk_size),
    ]
    for label, value in pairs:
        if value <= 0:
            raise ValueError(f"{label} 必须大于 0")
    if config.parent_chunk_overlap < 0 or config.child_chunk_overlap < 0:
        raise ValueError("chunk overlap 不能为负数")
    if config.parent_chunk_overlap >= config.parent_chunk_size:
        raise ValueError("parentChunkOverlap 必须小于 parentChunkSize")
    if config.child_chunk_overlap >= config.child_chunk_size:
        raise ValueError("childChunkOverlap 必须小于 childChunkSize")
