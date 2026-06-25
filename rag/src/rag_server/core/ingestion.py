"""RAG 导入任务管理。"""

from __future__ import annotations

import threading
import uuid
from copy import deepcopy
from dataclasses import dataclass, field
from pathlib import Path
from time import time

from pydantic import BaseModel, Field

from ..config import get_settings
from ..loaders import load_documents
from .chunking import build_child_documents
from .qdrant import index_documents


class IngestOptions(BaseModel):
    replace_existing: bool = Field(default=True, alias="replaceExisting")


class IngestRequest(BaseModel):
    project_id: str = Field(alias="projectId")
    project_path: str = Field(alias="projectPath")
    files: list[str]
    options: IngestOptions = Field(default_factory=IngestOptions)


@dataclass
class IngestFileState:
    path: str
    status: str = "pending"
    raw_documents: int = 0
    parent_chunks: int = 0
    child_chunks: int = 0
    indexed_points: int = 0
    error: str | None = None

    def to_dict(self) -> dict:
        return {
            "path": self.path,
            "status": self.status,
            "rawDocuments": self.raw_documents,
            "parentChunks": self.parent_chunks,
            "childChunks": self.child_chunks,
            "indexedPoints": self.indexed_points,
            "error": self.error,
        }


@dataclass
class IngestJob:
    id: str
    project_id: str
    project_path: str
    files: list[IngestFileState]
    status: str = "queued"
    total_files: int = 0
    completed_files: int = 0
    failed_files: int = 0
    created_at: float = field(default_factory=time)
    updated_at: float = field(default_factory=time)
    error: str | None = None

    def to_dict(self) -> dict:
        return {
            "jobId": self.id,
            "projectId": self.project_id,
            "status": self.status,
            "totalFiles": self.total_files,
            "completedFiles": self.completed_files,
            "failedFiles": self.failed_files,
            "createdAt": self.created_at,
            "updatedAt": self.updated_at,
            "error": self.error,
            "files": [item.to_dict() for item in self.files],
        }


_jobs: dict[str, IngestJob] = {}
_jobs_lock = threading.Lock()


def start_ingest_job(payload: IngestRequest) -> str:
    _validate_payload(payload)
    job_id = uuid.uuid4().hex
    job = IngestJob(
        id=job_id,
        project_id=payload.project_id,
        project_path=payload.project_path,
        files=[IngestFileState(path=str(Path(path).resolve())) for path in payload.files],
        total_files=len(payload.files),
    )
    with _jobs_lock:
        _jobs[job_id] = job
    thread = threading.Thread(target=_run_job, args=(job_id,), name=f"rag-ingest-{job_id[:8]}", daemon=True)
    thread.start()
    return job_id


def get_ingest_job(job_id: str) -> dict | None:
    with _jobs_lock:
        job = _jobs.get(job_id)
        return deepcopy(job.to_dict()) if job else None


def _run_job(job_id: str) -> None:
    _patch_job(job_id, status="running")
    with _jobs_lock:
        job = _jobs[job_id]
        project_id = job.project_id
        project_path = job.project_path
        file_count = len(job.files)

    for index in range(file_count):
        _process_file(job_id, index, project_id, project_path)

    with _jobs_lock:
        job = _jobs[job_id]
        if job.failed_files == 0:
            job.status = "done"
        elif job.completed_files > 0:
            job.status = "partial"
        else:
            job.status = "failed"
        job.updated_at = time()


def _process_file(job_id: str, index: int, project_id: str, project_path: str) -> None:
    _patch_file(job_id, index, status="running")
    with _jobs_lock:
        path = _jobs[job_id].files[index].path
    try:
        settings = get_settings()
        raw_docs = load_documents(path, project_id=project_id, project_path=project_path, ocr=settings.ocr)
        child_docs = build_child_documents(raw_docs, settings.chunking)
        parent_ids = {doc.metadata["parentId"] for doc in child_docs}
        indexed = index_documents(settings, project_id, path, child_docs)
        _patch_file(
            job_id,
            index,
            status="done",
            raw_documents=len(raw_docs),
            parent_chunks=len(parent_ids),
            child_chunks=len(child_docs),
            indexed_points=indexed,
        )
        _increment_job(job_id, completed=1)
    except Exception as exc:  # noqa: BLE001 - 需要把导入失败精确返回给 UI
        _patch_file(job_id, index, status="failed", error=str(exc))
        _increment_job(job_id, failed=1)


def _validate_payload(payload: IngestRequest) -> None:
    if not payload.project_id.strip():
        raise ValueError("projectId 不能为空")
    if not payload.files:
        raise ValueError("files 不能为空")
    project_root = Path(payload.project_path).resolve()
    if not project_root.is_dir():
        raise ValueError("projectPath 必须是存在的目录")
    for file in payload.files:
        path = Path(file).resolve()
        if not path.is_file():
            raise ValueError(f"文件不存在：{file}")
        if not path.is_relative_to(project_root):
            raise ValueError(f"文件不在项目目录内：{file}")


def _patch_job(job_id: str, **patch: object) -> None:
    with _jobs_lock:
        job = _jobs[job_id]
        for key, value in patch.items():
            setattr(job, key, value)
        job.updated_at = time()


def _patch_file(job_id: str, index: int, **patch: object) -> None:
    with _jobs_lock:
        file_state = _jobs[job_id].files[index]
        for key, value in patch.items():
            setattr(file_state, key, value)
        _jobs[job_id].updated_at = time()


def _increment_job(job_id: str, *, completed: int = 0, failed: int = 0) -> None:
    with _jobs_lock:
        job = _jobs[job_id]
        job.completed_files += completed
        job.failed_files += failed
        job.updated_at = time()
