from __future__ import annotations

import json
import re
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path

from .memory_store import summarize_for_memory

_COMPACT_CONTENT_LIMIT = 500


@dataclass(frozen=True)
class TranscriptStep:
    """A single recorded step in the transcript."""

    step_index: int
    timestamp: str
    source: str
    step_type: str
    status: str
    content: str
    tool_calls: tuple[dict[str, object], ...] = ()
    metadata: dict[str, object] = field(default_factory=dict)

    # -- serialisation --------------------------------------------------------

    def to_json_line(self) -> str:
        """Serialize this step to a single JSON string (one line)."""
        payload: dict[str, object] = {
            'step_index': self.step_index,
            'timestamp': self.timestamp,
            'source': self.source,
            'step_type': self.step_type,
            'status': self.status,
            'content': self.content,
            'tool_calls': [dict(tc) for tc in self.tool_calls],
            'metadata': dict(self.metadata),
        }
        return json.dumps(payload, ensure_ascii=False)

    @classmethod
    def from_json_line(cls, line: str) -> TranscriptStep:
        """Deserialize a JSON line into a *TranscriptStep*."""
        data = json.loads(line)
        return cls(
            step_index=int(data['step_index']),
            timestamp=str(data['timestamp']),
            source=str(data['source']),
            step_type=str(data['step_type']),
            status=str(data['status']),
            content=str(data['content']),
            tool_calls=tuple(dict(tc) for tc in data.get('tool_calls', [])),
            metadata=dict(data.get('metadata', {})),
        )


def _iso_now() -> str:
    """Return the current UTC time as an ISO 8601 string."""
    return datetime.now(timezone.utc).isoformat()


@dataclass
class TranscriptStore:
    """JSONL-backed transcript with backward-compatible string interface.

    Legacy callers that rely on ``entries``, ``memory_journal``, ``flushed``,
    ``append()``, ``remember()``, ``compact()``, ``replay()``,
    ``memory_digest()``, ``flush()``, and ``as_markdown()`` continue to work
    unchanged.

    New callers can use the richer ``record_step()`` API and the JSONL
    persistence layer.
    """

    # -- legacy fields (preserved for backward compatibility) ------------------
    entries: list[str] = field(default_factory=list)
    memory_journal: list[str] = field(default_factory=list)
    flushed: bool = False

    # -- new structured fields ------------------------------------------------
    steps: list[TranscriptStep] = field(default_factory=list)
    log_dir: Path | None = None
    _next_step_index: int = field(default=0, repr=False)

    # -- legacy interface -----------------------------------------------------

    def append(self, entry: str) -> None:
        """Append a plain-text entry **and** record a structured step."""
        self.entries.append(entry)
        self.flushed = False

        step = self._make_step(
            source='SYSTEM',
            step_type='USER_INPUT',
            content=entry,
            status='DONE',
        )
        self.steps.append(step)
        self._write_step_to_disk(step)

    def remember(self, *entries: str) -> None:
        for entry in entries:
            normalized = entry.strip()
            if normalized and normalized not in self.memory_journal:
                self.memory_journal.append(normalized)

    def compact(self, keep_last: int = 10) -> None:
        if len(self.entries) > keep_last:
            dropped = self.entries[:-keep_last]
            for entry in dropped:
                note = f'Compacted context: {summarize_for_memory([entry])}'
                if note not in self.memory_journal:
                    self.memory_journal.append(note)
            self.entries[:] = self.entries[-keep_last:]

    def replay(self, include_memory: bool = True) -> tuple[str, ...]:
        if include_memory:
            return tuple(self.memory_journal + self.entries)
        return tuple(self.entries)

    def memory_digest(self, limit: int = 10) -> tuple[str, ...]:
        return tuple(self.memory_journal[-limit:])

    def flush(self) -> None:
        """Mark the store as flushed and persist all steps to JSONL."""
        self.flushed = True
        if self.log_dir is not None:
            self.write_jsonl(self.log_dir / 'transcript.jsonl')

    def as_markdown(self, limit: int = 10) -> str:
        lines = ['# Transcript', '']
        lines.append(f'Flushed: {self.flushed}')
        lines.append(f'Current entries: {len(self.entries)}')
        lines.append(f'Memory notes: {len(self.memory_journal)}')
        lines.append(f'Recorded steps: {len(self.steps)}')
        if self.memory_journal:
            lines.extend(['', 'Memory digest:'])
            lines.extend(f'- {entry}' for entry in self.memory_digest(limit))
        if self.entries:
            lines.extend(['', 'Recent entries:'])
            lines.extend(f'- {entry}' for entry in self.entries[-limit:])
        return '\n'.join(lines)

    # -- new structured API ---------------------------------------------------

    def record_step(
        self,
        source: str,
        step_type: str,
        content: str,
        status: str = 'DONE',
        tool_calls: tuple[dict[str, object], ...] = (),
        metadata: dict[str, object] | None = None,
    ) -> TranscriptStep:
        """Record a fully-detailed transcript step."""
        step = self._make_step(
            source=source,
            step_type=step_type,
            content=content,
            status=status,
            tool_calls=tool_calls,
            metadata=metadata or {},
        )
        self.steps.append(step)
        self._write_step_to_disk(step)
        return step

    # -- JSONL persistence ----------------------------------------------------

    def write_jsonl(self, path: Path) -> None:
        """Write all recorded steps to a JSONL file at *path*.

        A compact variant (content truncated to 500 chars) is also written
        alongside as ``transcript_compact.jsonl`` in the same directory.
        """
        path = Path(path)
        path.parent.mkdir(parents=True, exist_ok=True)

        with path.open('w', encoding='utf-8') as fh:
            for step in self.steps:
                fh.write(step.to_json_line() + '\n')

        compact_path = path.parent / 'transcript_compact.jsonl'
        with compact_path.open('w', encoding='utf-8') as fh:
            for step in self.steps:
                truncated_content = step.content[:_COMPACT_CONTENT_LIMIT]
                compact_step = TranscriptStep(
                    step_index=step.step_index,
                    timestamp=step.timestamp,
                    source=step.source,
                    step_type=step.step_type,
                    status=step.status,
                    content=truncated_content,
                    tool_calls=step.tool_calls,
                    metadata=step.metadata,
                )
                fh.write(compact_step.to_json_line() + '\n')

    @classmethod
    def load_jsonl(cls, path: Path) -> TranscriptStore:
        """Load a *TranscriptStore* from a JSONL file."""
        path = Path(path)
        loaded_steps: list[TranscriptStep] = []
        with path.open('r', encoding='utf-8') as fh:
            for line in fh:
                stripped = line.strip()
                if stripped:
                    loaded_steps.append(TranscriptStep.from_json_line(stripped))

        store = cls(
            log_dir=path.parent,
            steps=loaded_steps,
        )
        # Restore the entry list from recorded steps so legacy callers work.
        store.entries = [s.content for s in loaded_steps]
        # Set the next index past the highest loaded index.
        if loaded_steps:
            store._next_step_index = max(s.step_index for s in loaded_steps) + 1
        return store

    # -- search / filter ------------------------------------------------------

    def search(self, query: str) -> list[TranscriptStep]:
        """Case-insensitive substring search across step content."""
        query_lower = query.lower()
        return [s for s in self.steps if query_lower in s.content.lower()]

    def grep(self, pattern: str) -> list[TranscriptStep]:
        """Regex search across step content."""
        compiled = re.compile(pattern)
        return [s for s in self.steps if compiled.search(s.content)]

    def get_steps_by_type(self, step_type: str) -> list[TranscriptStep]:
        """Return all steps matching the given *step_type*."""
        return [s for s in self.steps if s.step_type == step_type]

    def get_steps_by_source(self, source: str) -> list[TranscriptStep]:
        """Return all steps matching the given *source*."""
        return [s for s in self.steps if s.source == source]

    # -- internal helpers -----------------------------------------------------

    def _make_step(
        self,
        source: str,
        step_type: str,
        content: str,
        status: str,
        tool_calls: tuple[dict[str, object], ...] = (),
        metadata: dict[str, object] | None = None,
    ) -> TranscriptStep:
        idx = self._next_step_index
        self._next_step_index += 1
        return TranscriptStep(
            step_index=idx,
            timestamp=_iso_now(),
            source=source,
            step_type=step_type,
            status=status,
            content=content,
            tool_calls=tool_calls,
            metadata=metadata or {},
        )

    def _write_step_to_disk(self, step: TranscriptStep) -> None:
        """Append a single step to the on-disk JSONL if *log_dir* is set."""
        if self.log_dir is None:
            return
        jsonl_path = self.log_dir / 'transcript.jsonl'
        jsonl_path.parent.mkdir(parents=True, exist_ok=True)
        with jsonl_path.open('a', encoding='utf-8') as fh:
            fh.write(step.to_json_line() + '\n')
