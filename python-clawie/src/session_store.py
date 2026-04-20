from __future__ import annotations

import json
from dataclasses import asdict, dataclass
from pathlib import Path


@dataclass(frozen=True)
class StoredSession:
    session_id: str
    messages: tuple[str, ...]
    input_tokens: int
    output_tokens: int


class SessionStoreError(RuntimeError):
    pass


DEFAULT_SESSION_DIR = Path('.port_sessions')


def save_session(session: StoredSession, directory: Path | None = None) -> Path:
    target_dir = directory or DEFAULT_SESSION_DIR
    path = target_dir / f'{session.session_id}.json'
    try:
        target_dir.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps(asdict(session), indent=2), encoding='utf-8')
    except OSError as exc:
        raise SessionStoreError(f"Failed to save session '{session.session_id}' to {path}: {exc}") from exc
    return path


def load_session(session_id: str, directory: Path | None = None) -> StoredSession:
    target_dir = directory or DEFAULT_SESSION_DIR
    path = target_dir / f'{session_id}.json'
    try:
        raw = path.read_text(encoding='utf-8')
        data = json.loads(raw)
    except FileNotFoundError as exc:
        raise SessionStoreError(f"Session '{session_id}' was not found at {path}") from exc
    except json.JSONDecodeError as exc:
        raise SessionStoreError(
            f"Session '{session_id}' is corrupted at {path}: {exc.msg} (line {exc.lineno}, column {exc.colno})"
        ) from exc
    except OSError as exc:
        raise SessionStoreError(f"Failed to read session '{session_id}' from {path}: {exc}") from exc
    try:
        messages = tuple(data['messages'])
        input_tokens = data['input_tokens']
        output_tokens = data['output_tokens']
        stored_session_id = data['session_id']
    except KeyError as exc:
        raise SessionStoreError(f"Session '{session_id}' is missing required field: {exc.args[0]}") from exc
    return StoredSession(
        session_id=stored_session_id,
        messages=messages,
        input_tokens=input_tokens,
        output_tokens=output_tokens,
    )
