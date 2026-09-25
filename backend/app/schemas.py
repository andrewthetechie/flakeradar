"""Pydantic response models — the typed contract the frontend and MCP consume."""
from datetime import datetime
from typing import Any

from pydantic import BaseModel


class IngestAccepted(BaseModel):
    report_id: int
    status: str  # always "pending"


class ReportOut(BaseModel):
    id: int
    repo: str
    project: str
    commit_sha: str
    branch: str
    ci_run_id: str
    status: str  # pending | processed | failed
    error: str | None
    counts: dict[str, Any] | None
    run_id: int | None
    created_at: datetime
    processed_at: datetime | None


class ReportSummaryOut(BaseModel):
    pending: int
    failed: int
