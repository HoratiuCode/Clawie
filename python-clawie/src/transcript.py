from __future__ import annotations

from dataclasses import dataclass, field

from .memory_store import summarize_for_memory


@dataclass
class TranscriptStore:
    entries: list[str] = field(default_factory=list)
    memory_journal: list[str] = field(default_factory=list)
    flushed: bool = False

    def append(self, entry: str) -> None:
        self.entries.append(entry)
        self.flushed = False

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
        self.flushed = True

    def as_markdown(self, limit: int = 10) -> str:
        lines = ['# Transcript', '']
        lines.append(f'Flushed: {self.flushed}')
        lines.append(f'Current entries: {len(self.entries)}')
        lines.append(f'Memory notes: {len(self.memory_journal)}')
        if self.memory_journal:
            lines.extend(['', 'Memory digest:'])
            lines.extend(f'- {entry}' for entry in self.memory_digest(limit))
        if self.entries:
            lines.extend(['', 'Recent entries:'])
            lines.extend(f'- {entry}' for entry in self.entries[-limit:])
        return '\n'.join(lines)
