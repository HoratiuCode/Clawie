from __future__ import annotations

import json
import re
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Iterable, Sequence


MEMORY_STORE_PATH = Path('.port_sessions') / 'workspace_memory.json'
_BACKTICK_PATTERN = re.compile(r'`([^`]{2,160})`')
_PATH_PATTERN = re.compile(
    r'(?:(?:[A-Za-z]:[\\/])|(?:~?/)|(?:\.\./)|(?:\./)|(?:[A-Za-z0-9_.-]+/))+[A-Za-z0-9_.-]+'
)


def _dedupe(items: Iterable[str]) -> tuple[str, ...]:
    seen: set[str] = set()
    ordered: list[str] = []
    for item in items:
        normalized = item.strip()
        if not normalized or normalized in seen:
            continue
        seen.add(normalized)
        ordered.append(normalized)
    return tuple(ordered)


def _preview_text(text: str, max_words: int = 16) -> str:
    words = text.split()
    if len(words) <= max_words:
        return text.strip()
    return ' '.join(words[:max_words]).rstrip() + '...'


def extract_code_references(text: str) -> tuple[str, ...]:
    references: list[str] = []
    for match in _BACKTICK_PATTERN.findall(text):
        references.append(match)
    for match in _PATH_PATTERN.findall(text):
        references.append(match)
    return _dedupe(references)


def summarize_for_memory(lines: Sequence[str]) -> str:
    joined = ' | '.join(line.strip() for line in lines if line.strip())
    return _preview_text(joined)


@dataclass(frozen=True)
class WorkspaceMemory:
    notes: tuple[str, ...] = ()
    code_references: tuple[str, ...] = ()
    session_ids: tuple[str, ...] = ()

    def as_markdown(self, limit: int = 10) -> str:
        lines = [
            '# Workspace Memory',
            '',
            f'Sessions tracked: {len(self.session_ids)}',
            f'Notes stored: {len(self.notes)}',
            f'Code references stored: {len(self.code_references)}',
        ]
        if self.code_references:
            lines.extend(['', 'Code references:'])
            lines.extend(f'- {reference}' for reference in self.code_references[-limit:])
        if self.notes:
            lines.extend(['', 'Memory notes:'])
            lines.extend(f'- {note}' for note in self.notes[-limit:])
        return '\n'.join(lines)


def load_workspace_memory(directory: Path | None = None) -> WorkspaceMemory:
    path = (directory or MEMORY_STORE_PATH.parent) / MEMORY_STORE_PATH.name
    if not path.exists():
        return WorkspaceMemory()
    data = json.loads(path.read_text(encoding='utf-8'))
    return WorkspaceMemory(
        notes=tuple(data.get('notes', ())),
        code_references=tuple(data.get('code_references', ())),
        session_ids=tuple(data.get('session_ids', ())),
    )


def save_workspace_memory(memory: WorkspaceMemory, directory: Path | None = None) -> Path:
    path = (directory or MEMORY_STORE_PATH.parent) / MEMORY_STORE_PATH.name
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(asdict(memory), indent=2), encoding='utf-8')
    return path


def merge_workspace_memory(
    memory: WorkspaceMemory,
    session_id: str,
    summary_lines: Sequence[str],
    text_blobs: Sequence[str] = (),
) -> WorkspaceMemory:
    notes = list(memory.notes)
    note = summarize_for_memory(summary_lines)
    if note and note not in notes:
        notes.append(note)
    for blob in text_blobs:
        preview = _preview_text(blob)
        if preview and preview not in notes:
            notes.append(preview)
    code_references = list(memory.code_references)
    for blob in list(summary_lines) + list(text_blobs):
        for reference in extract_code_references(blob):
            if reference not in code_references:
                code_references.append(reference)
    session_ids = list(memory.session_ids)
    if session_id not in session_ids:
        session_ids.append(session_id)
    return WorkspaceMemory(
        notes=_dedupe(notes),
        code_references=_dedupe(code_references),
        session_ids=_dedupe(session_ids),
    )
