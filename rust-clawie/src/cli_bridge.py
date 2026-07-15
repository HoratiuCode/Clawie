from __future__ import annotations

import os
import shlex
import shutil
import subprocess
from dataclasses import dataclass, field
from pathlib import Path


@dataclass(frozen=True)
class CliBridgeRequest:
    target: str
    command: tuple[str, ...]
    prompt: str | None = None
    env: tuple[tuple[str, str], ...] = ()
    cwd: str | None = None
    timeout_seconds: float = 120.0
    execute: bool = False


@dataclass(frozen=True)
class CliBridgeReport:
    mode: str
    target: str
    transport: str
    command: tuple[str, ...]
    command_found: bool
    connected: bool
    executed: bool
    exit_code: int | None = None
    stdout: str = ''
    stderr: str = ''
    detail: str = ''
    env_keys: tuple[str, ...] = field(default_factory=tuple)

    def as_text(self) -> str:
        lines = [
            f'mode={self.mode}',
            f'target={self.target}',
            f'transport={self.transport}',
            f'command={shlex.join(self.command)}',
            f'command_found={self.command_found}',
            f'connected={self.connected}',
            f'executed={self.executed}',
        ]
        if self.exit_code is not None:
            lines.append(f'exit_code={self.exit_code}')
        if self.env_keys:
            lines.append(f'env_keys={",".join(self.env_keys)}')
        if self.detail:
            lines.append(f'detail={self.detail}')
        if self.stdout:
            lines.extend(['stdout<<EOF', self.stdout.rstrip(), 'EOF'])
        if self.stderr:
            lines.extend(['stderr<<EOF', self.stderr.rstrip(), 'EOF'])
        return '\n'.join(lines)


def parse_command(command: str | tuple[str, ...] | list[str]) -> tuple[str, ...]:
    if isinstance(command, str):
        parsed = tuple(shlex.split(command))
    else:
        parsed = tuple(str(part) for part in command)
    if not parsed:
        raise ValueError('CLI bridge command cannot be empty')
    return parsed


def parse_env_assignments(assignments: list[str] | tuple[str, ...]) -> tuple[tuple[str, str], ...]:
    parsed: list[tuple[str, str]] = []
    for assignment in assignments:
        if '=' not in assignment:
            raise ValueError(f'env assignment must use KEY=VALUE syntax: {assignment}')
        key, value = assignment.split('=', 1)
        key = key.strip()
        if not key:
            raise ValueError(f'env assignment has empty key: {assignment}')
        parsed.append((key, value))
    return tuple(parsed)


def build_cli_bridge_request(
    target: str,
    command: str | tuple[str, ...] | list[str] | None = None,
    *,
    extra_args: tuple[str, ...] = (),
    prompt: str | None = None,
    env: tuple[tuple[str, str], ...] = (),
    cwd: str | None = None,
    timeout_seconds: float = 120.0,
    execute: bool = False,
) -> CliBridgeRequest:
    base_command = command or os.environ.get('CLAWIE_INFRA_CLI') or target
    parsed_command = parse_command(base_command) + tuple(extra_args)
    if prompt:
        parsed_command = parsed_command + (prompt,)
    return CliBridgeRequest(
        target=target,
        command=parsed_command,
        prompt=prompt,
        env=env,
        cwd=cwd,
        timeout_seconds=timeout_seconds,
        execute=execute,
    )


def run_cli_bridge(request: CliBridgeRequest) -> CliBridgeReport:
    executable = request.command[0]
    command_found = Path(executable).exists() if os.path.sep in executable else shutil.which(executable) is not None
    if not command_found:
        return CliBridgeReport(
            mode='cli-bridge',
            target=request.target,
            transport='stdio-cli',
            command=request.command,
            command_found=False,
            connected=False,
            executed=False,
            detail=f'CLI executable not found: {executable}',
            env_keys=tuple(key for key, _value in request.env),
        )

    if not request.execute:
        return CliBridgeReport(
            mode='cli-bridge',
            target=request.target,
            transport='stdio-cli',
            command=request.command,
            command_found=True,
            connected=True,
            executed=False,
            detail='CLI transport is available; pass --execute to run it',
            env_keys=tuple(key for key, _value in request.env),
        )

    env = os.environ.copy()
    env.update(dict(request.env))
    completed = subprocess.run(
        request.command,
        cwd=request.cwd,
        env=env,
        capture_output=True,
        text=True,
        timeout=request.timeout_seconds,
        check=False,
    )
    return CliBridgeReport(
        mode='cli-bridge',
        target=request.target,
        transport='stdio-cli',
        command=request.command,
        command_found=True,
        connected=completed.returncode == 0,
        executed=True,
        exit_code=completed.returncode,
        stdout=completed.stdout,
        stderr=completed.stderr,
        detail='CLI transport executed',
        env_keys=tuple(key for key, _value in request.env),
    )


def run_cli_bridge_mode(
    target: str,
    command: str | None = None,
    *,
    extra_args: tuple[str, ...] = (),
    prompt: str | None = None,
    env: tuple[tuple[str, str], ...] = (),
    cwd: str | None = None,
    timeout_seconds: float = 120.0,
    execute: bool = False,
) -> CliBridgeReport:
    request = build_cli_bridge_request(
        target,
        command,
        extra_args=extra_args,
        prompt=prompt,
        env=env,
        cwd=cwd,
        timeout_seconds=timeout_seconds,
        execute=execute,
    )
    return run_cli_bridge(request)
