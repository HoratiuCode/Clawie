"""Artifact store for structured, persistent outputs.

Inspired by Antigravity / Gemini CLI artifact system.  Artifacts are
markdown (or arbitrary text) documents that live on disk inside a
session directory.  A JSON manifest (`_index.json`) tracks every
artifact's metadata so the store can be re-hydrated across restarts.
"""

from __future__ import annotations

import json
from dataclasses import dataclass, field, replace
from datetime import datetime, timezone
from pathlib import Path
from typing import Any
from uuid import uuid4


# ---------------------------------------------------------------------------
# Data models
# ---------------------------------------------------------------------------

@dataclass(frozen=True)
class ArtifactMetadata:
    """Immutable metadata attached to every artifact."""

    summary: str
    user_facing: bool = True
    request_feedback: bool = False
    created_at: str = ""
    updated_at: str = ""
    tags: tuple[str, ...] = ()

    def to_dict(self) -> dict[str, Any]:
        return {
            "summary": self.summary,
            "user_facing": self.user_facing,
            "request_feedback": self.request_feedback,
            "created_at": self.created_at,
            "updated_at": self.updated_at,
            "tags": list(self.tags),
        }

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> ArtifactMetadata:
        return cls(
            summary=data["summary"],
            user_facing=data.get("user_facing", True),
            request_feedback=data.get("request_feedback", False),
            created_at=data.get("created_at", ""),
            updated_at=data.get("updated_at", ""),
            tags=tuple(data.get("tags", ())),
        )


@dataclass(frozen=True)
class Artifact:
    """Immutable handle to a single artifact on disk."""

    artifact_id: str
    name: str
    content: str
    metadata: ArtifactMetadata
    path: str

    def to_dict(self) -> dict[str, Any]:
        return {
            "artifact_id": self.artifact_id,
            "name": self.name,
            "metadata": self.metadata.to_dict(),
            "path": self.path,
        }

    @classmethod
    def from_dict(cls, data: dict[str, Any], content: str) -> Artifact:
        return cls(
            artifact_id=data["artifact_id"],
            name=data["name"],
            content=content,
            metadata=ArtifactMetadata.from_dict(data["metadata"]),
            path=data["path"],
        )


# ---------------------------------------------------------------------------
# Store
# ---------------------------------------------------------------------------

class ArtifactStore:
    """Manages artifacts for a single session on the local filesystem.

    Directory layout::

        {base_dir}/{session_id}/
            _index.json          ← manifest of all artifacts
            analysis_results.md  ← artifact file
            scratch/             ← throwaway / non-user-facing files
                debug_script.py
    """

    def __init__(self, base_dir: str | Path, session_id: str) -> None:
        self.base_dir = Path(base_dir)
        self.session_id = session_id

        self._session_dir = self.base_dir / self.session_id
        self._scratch_dir = self._session_dir / "scratch"
        self._index_path = self._session_dir / "_index.json"

        # artifact_id → serialisable dict (without content)
        self._index: dict[str, dict[str, Any]] = {}

        self._ensure_dirs()
        self._load_index()

    # -- public API ---------------------------------------------------------

    def create(
        self,
        name: str,
        content: str,
        summary: str,
        *,
        user_facing: bool = True,
        request_feedback: bool = False,
        tags: tuple[str, ...] = (),
    ) -> Artifact:
        """Create a new artifact, write it to disk, and return it."""
        now = datetime.now(timezone.utc).isoformat()
        artifact_id = uuid4().hex

        artifact_path = self._session_dir / name
        metadata = ArtifactMetadata(
            summary=summary,
            user_facing=user_facing,
            request_feedback=request_feedback,
            created_at=now,
            updated_at=now,
            tags=tags,
        )
        artifact = Artifact(
            artifact_id=artifact_id,
            name=name,
            content=content,
            metadata=metadata,
            path=str(artifact_path),
        )

        artifact_path.parent.mkdir(parents=True, exist_ok=True)
        artifact_path.write_text(content, encoding="utf-8")

        self._index[artifact_id] = artifact.to_dict()
        self._save_index()

        return artifact

    def update(
        self,
        artifact_id: str,
        content: str | None = None,
        summary: str | None = None,
    ) -> Artifact:
        """Update an existing artifact's content and/or summary.

        Raises ``KeyError`` if the artifact does not exist.
        """
        if artifact_id not in self._index:
            raise KeyError(f"Unknown artifact: {artifact_id}")

        entry = self._index[artifact_id]
        old_metadata = ArtifactMetadata.from_dict(entry["metadata"])
        artifact_path = Path(entry["path"])

        now = datetime.now(timezone.utc).isoformat()
        new_metadata = replace(
            old_metadata,
            updated_at=now,
            summary=summary if summary is not None else old_metadata.summary,
        )

        if content is None:
            content = artifact_path.read_text(encoding="utf-8")
        else:
            artifact_path.write_text(content, encoding="utf-8")

        artifact = Artifact(
            artifact_id=artifact_id,
            name=entry["name"],
            content=content,
            metadata=new_metadata,
            path=str(artifact_path),
        )

        self._index[artifact_id] = artifact.to_dict()
        self._save_index()

        return artifact

    def get(self, artifact_id: str) -> Artifact | None:
        """Return the artifact with *artifact_id*, or ``None``."""
        entry = self._index.get(artifact_id)
        if entry is None:
            return None
        return self._hydrate(entry)

    def get_by_name(self, name: str) -> Artifact | None:
        """Return the first artifact whose *name* matches, or ``None``."""
        for entry in self._index.values():
            if entry["name"] == name:
                return self._hydrate(entry)
        return None

    def list_artifacts(self, *, user_facing_only: bool = False) -> list[Artifact]:
        """Return all artifacts, optionally filtered to user-facing ones."""
        results: list[Artifact] = []
        for entry in self._index.values():
            if user_facing_only and not entry["metadata"].get("user_facing", True):
                continue
            artifact = self._hydrate(entry)
            if artifact is not None:
                results.append(artifact)
        return results

    def delete(self, artifact_id: str) -> None:
        """Remove an artifact from disk and the index.

        Raises ``KeyError`` if the artifact does not exist.
        """
        if artifact_id not in self._index:
            raise KeyError(f"Unknown artifact: {artifact_id}")

        entry = self._index.pop(artifact_id)
        artifact_path = Path(entry["path"])
        if artifact_path.exists():
            artifact_path.unlink()

        self._save_index()

    def create_scratch(self, name: str, content: str) -> Artifact:
        """Create a scratch (non-user-facing) file in the scratch sub-dir."""
        scratch_name = f"scratch/{name}"
        return self.create(
            name=scratch_name,
            content=content,
            summary=f"Scratch file: {name}",
            user_facing=False,
            request_feedback=False,
        )

    def as_markdown(self) -> str:
        """Return a formatted markdown summary of every artifact."""
        artifacts = self.list_artifacts()
        if not artifacts:
            return "_No artifacts in this session._"

        lines: list[str] = ["# Artifacts", ""]
        for art in artifacts:
            visibility = "👤 user-facing" if art.metadata.user_facing else "🔧 internal"
            feedback = " · 🗳 feedback requested" if art.metadata.request_feedback else ""
            tags = ", ".join(art.metadata.tags) if art.metadata.tags else "—"
            lines.extend([
                f"## {art.name}",
                "",
                f"| Field | Value |",
                f"|-------|-------|",
                f"| ID | `{art.artifact_id}` |",
                f"| Path | `{art.path}` |",
                f"| Visibility | {visibility}{feedback} |",
                f"| Tags | {tags} |",
                f"| Created | {art.metadata.created_at} |",
                f"| Updated | {art.metadata.updated_at} |",
                "",
                f"**Summary:** {art.metadata.summary}",
                "",
            ])

        return "\n".join(lines)

    # -- private helpers ----------------------------------------------------

    def _ensure_dirs(self) -> None:
        self._session_dir.mkdir(parents=True, exist_ok=True)
        self._scratch_dir.mkdir(parents=True, exist_ok=True)

    def _load_index(self) -> None:
        if self._index_path.exists():
            raw = self._index_path.read_text(encoding="utf-8")
            self._index = json.loads(raw)
        else:
            self._index = {}

    def _save_index(self) -> None:
        self._index_path.write_text(
            json.dumps(self._index, indent=2, ensure_ascii=False) + "\n",
            encoding="utf-8",
        )

    def _hydrate(self, entry: dict[str, Any]) -> Artifact | None:
        """Reconstruct an ``Artifact`` from an index entry + file on disk."""
        artifact_path = Path(entry["path"])
        if not artifact_path.exists():
            return None
        content = artifact_path.read_text(encoding="utf-8")
        return Artifact.from_dict(entry, content)
