"""文档导入接口。"""

from __future__ import annotations

from fastapi import APIRouter, HTTPException

from ..core.ingestion import IngestRequest, get_ingest_job, start_ingest_job

router = APIRouter(prefix="/ingest", tags=["ingest"])


@router.post("/jobs")
def create_ingest_job(payload: IngestRequest) -> dict:
    try:
        job_id = start_ingest_job(payload)
    except ValueError as exc:
        raise HTTPException(status_code=400, detail=str(exc)) from exc
    return {"jobId": job_id}


@router.get("/jobs/{job_id}")
def read_ingest_job(job_id: str) -> dict:
    job = get_ingest_job(job_id)
    if job is None:
        raise HTTPException(status_code=404, detail="导入任务不存在")
    return job
