from __future__ import annotations

import ast
import re
from dataclasses import dataclass
from pathlib import Path


DEFAULT_SCAN_ROOT = Path('.')
_MARKER_PATTERN = re.compile(r'\b(TODO|FIXME)\b', re.IGNORECASE)
_BROAD_EXCEPTION_MESSAGE = 'Broad exception handler should be narrowed and preserve context'
_TODO_MESSAGE = 'Marker should be resolved before ship'


@dataclass(frozen=True)
class ShrimpiFinding:
    path: str
    line: int
    category: str
    severity: str
    message: str
    suggestion: str


@dataclass(frozen=True)
class ShrimpiReport:
    roots: tuple[str, ...]
    scanned_files: int
    clean_files: int
    findings: tuple[ShrimpiFinding, ...]
    passed_checks: tuple[str, ...]
    blocked_checks: tuple[str, ...]

    @property
    def ready_to_ship(self) -> bool:
        return not self.findings

    def as_markdown(self, limit: int = 20) -> str:
        lines = [
            '# Shrimpi Report',
            '',
            '## Pre-ship',
            f'- Scan roots: {", ".join(self.roots) if self.roots else "none"}',
            f'- Files scanned: {self.scanned_files}',
            f'- Clean files: {self.clean_files}',
            f'- Findings: {len(self.findings)}',
            f'- Ship-ready: {self.ready_to_ship}',
            '',
            '## Findings',
        ]
        if self.findings:
            for finding in self.findings[:limit]:
                lines.append(
                    f'- `{finding.path}:{finding.line}` [{finding.severity}/{finding.category}] {finding.message}'
                    f' -> {finding.suggestion}'
                )
        else:
            lines.append('- none')
        lines.extend(['', '## Checks Passed'])
        if self.passed_checks:
            lines.extend(f'- {check}' for check in self.passed_checks)
        else:
            lines.append('- none')
        suggestions = _dedupe(
            finding.suggestion for finding in self.findings if finding.suggestion
        )
        lines.extend(['', '## Recommended Fixes'])
        if suggestions:
            lines.extend(f'- {suggestion}' for suggestion in suggestions[:limit])
        else:
            lines.append('- none')
        lines.extend(['', '## Status'])
        if self.ready_to_ship:
            lines.append('- Ready for ship: yes')
        else:
            lines.append('- Ready for ship: no')
        if self.blocked_checks:
            lines.extend(['', 'Blocked checks:'])
            lines.extend(f'- {check}' for check in self.blocked_checks)
        return '\n'.join(lines)


def discover_scan_roots(root: Path) -> tuple[Path, ...]:
    if root.is_file():
        return (root,)
    roots: list[Path] = []
    src_root = root / 'src'
    if src_root.is_dir():
        roots.append(src_root)
    python_root = root / 'python-clawie' / 'src'
    if python_root.is_dir():
        roots.append(python_root)
    rust_root = root / 'rust-clawie' / 'src'
    if rust_root.is_dir():
        roots.append(rust_root)
    if roots:
        return _dedupe_paths(roots)
    return (root,)


def scan_workspace(target: Path | str | None = None) -> ShrimpiReport:
    root = Path(target).resolve() if target is not None else Path.cwd().resolve()
    scan_roots = discover_scan_roots(root)
    findings: list[ShrimpiFinding] = []
    clean_files = 0
    seen_files: set[Path] = set()
    blocked_checks: set[str] = set()
    passed_checks: set[str] = set()
    syntax_ok = True
    markers_ok = True
    error_handling_ok = True
    simplicity_ok = True
    for scan_root in scan_roots:
        for path in sorted(scan_root.rglob('*.py')):
            if not path.is_file() or '__pycache__' in path.parts:
                continue
            if path in seen_files:
                continue
            seen_files.add(path)
            file_findings, file_checks = _scan_file(path, root)
            findings.extend(file_findings)
            if not file_findings:
                clean_files += 1
            syntax_ok = syntax_ok and file_checks['syntax_ok']
            markers_ok = markers_ok and file_checks['markers_ok']
            error_handling_ok = error_handling_ok and file_checks['error_handling_ok']
            simplicity_ok = simplicity_ok and file_checks['simplicity_ok']
            if not file_checks['syntax_ok']:
                blocked_checks.add('syntax')
            if not file_checks['markers_ok']:
                blocked_checks.add('markers')
            if not file_checks['error_handling_ok']:
                blocked_checks.add('error-handling')
            if not file_checks['simplicity_ok']:
                blocked_checks.add('simplicity')
    if syntax_ok:
        passed_checks.add('No syntax errors detected in scanned Python files.')
    if markers_ok:
        passed_checks.add('No TODO or FIXME markers detected in scanned Python files.')
    if error_handling_ok:
        passed_checks.add('No broad exception handlers detected in scanned Python files.')
    if simplicity_ok:
        passed_checks.add('No oversized or highly complex functions detected.')
    return ShrimpiReport(
        roots=tuple(_label_path(path, root) for path in scan_roots),
        scanned_files=len(seen_files),
        clean_files=clean_files,
        findings=tuple(findings),
        passed_checks=tuple(sorted(passed_checks)),
        blocked_checks=tuple(sorted(blocked_checks)),
    )


def _scan_file(path: Path, base_root: Path) -> tuple[list[ShrimpiFinding], dict[str, bool]]:
    findings: list[ShrimpiFinding] = []
    syntax_ok = True
    markers_ok = True
    error_handling_ok = True
    simplicity_ok = True
    try:
        text = path.read_text(encoding='utf-8')
    except OSError as exc:
        findings.append(
            ShrimpiFinding(
                path=_label_path(path, base_root),
                line=1,
                category='io',
                severity='critical',
                message=f'File could not be read: {exc}',
                suggestion='Restore file access or rewrite the file before ship.',
            )
        )
        return findings, {
            'syntax_ok': False,
            'markers_ok': False,
            'error_handling_ok': False,
            'simplicity_ok': False,
        }

    lines = text.splitlines()
    for index, line in enumerate(lines, start=1):
        if _MARKER_PATTERN.search(line):
            markers_ok = False
            findings.append(
                ShrimpiFinding(
                    path=_label_path(path, base_root),
                    line=index,
                    category='marker',
                    severity='warning',
                    message='TODO/FIXME marker found.',
                    suggestion='Resolve the marker or convert it into a tracked issue.',
                )
            )

    try:
        tree = ast.parse(text, filename=str(path))
    except SyntaxError as exc:
        syntax_ok = False
        findings.append(
            ShrimpiFinding(
                path=_label_path(path, base_root),
                line=exc.lineno or 1,
                category='syntax',
                severity='critical',
                message=f'Syntax error: {exc.msg}',
                suggestion='Fix the syntax error before ship.',
            )
        )
        return findings, {
            'syntax_ok': syntax_ok,
            'markers_ok': markers_ok,
            'error_handling_ok': False,
            'simplicity_ok': False,
        }

    complexity_findings, complexity_ok = _inspect_tree(tree, path, base_root)
    findings.extend(complexity_findings)
    simplicity_ok = simplicity_ok and complexity_ok
    broad_findings = _inspect_error_handlers(tree, path, base_root)
    if broad_findings:
        error_handling_ok = False
        findings.extend(broad_findings)
    return findings, {
        'syntax_ok': syntax_ok,
        'markers_ok': markers_ok,
        'error_handling_ok': error_handling_ok,
        'simplicity_ok': simplicity_ok,
    }


def _inspect_tree(tree: ast.AST, path: Path, base_root: Path) -> tuple[list[ShrimpiFinding], bool]:
    findings: list[ShrimpiFinding] = []
    ok = True
    for node in ast.walk(tree):
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
            span = _node_span(node)
            complexity = _cyclomatic_complexity(node)
            if span >= 90:
                ok = False
                findings.append(
                    ShrimpiFinding(
                        path=_label_path(path, base_root),
                        line=node.lineno,
                        category='simplicity',
                        severity='warning',
                        message=f'Function spans {span} lines.',
                        suggestion='Split the function into smaller helpers and keep the main path linear.',
                    )
                )
            if complexity >= 14:
                ok = False
                findings.append(
                    ShrimpiFinding(
                        path=_label_path(path, base_root),
                        line=node.lineno,
                        category='efficiency',
                        severity='warning',
                        message=f'Function has estimated complexity {complexity}.',
                        suggestion='Reduce branching and reuse shared logic to keep the code easier to maintain.',
                    )
                )
    return findings, ok


def _inspect_error_handlers(tree: ast.AST, path: Path, base_root: Path) -> list[ShrimpiFinding]:
    findings: list[ShrimpiFinding] = []
    for node in ast.walk(tree):
        if not isinstance(node, ast.ExceptHandler):
            continue
        if _is_allowed_main_wrapper(path, node):
            continue
        if node.type is None or (isinstance(node.type, ast.Name) and node.type.id == 'Exception'):
            findings.append(
                ShrimpiFinding(
                    path=_label_path(path, base_root),
                    line=node.lineno,
                    category='error-handling',
                    severity='warning',
                    message='Broad exception handler found.',
                    suggestion='Narrow the exception type and keep the error context in the message.',
                )
            )
    return findings


def _is_allowed_main_wrapper(path: Path, node: ast.ExceptHandler) -> bool:
    if path.name != 'main.py':
        return False
    if node.type is None:
        return False
    if not (isinstance(node.type, ast.Name) and node.type.id == 'Exception'):
        return False
    if len(node.body) != 1:
        return False
    stmt = node.body[0]
    if not isinstance(stmt, ast.Return):
        return False
    call = stmt.value
    if not isinstance(call, ast.Call):
        return False
    if not isinstance(call.func, ast.Name) or call.func.id != '_print_error':
        return False
    if len(call.args) != 1:
        return False
    return isinstance(call.args[0], ast.Name) and call.args[0].id == 'exc'


def _cyclomatic_complexity(node: ast.AST) -> int:
    complexity = 1
    for child in ast.walk(node):
        if isinstance(child, (ast.If, ast.For, ast.AsyncFor, ast.While, ast.Try, ast.With, ast.AsyncWith, ast.ExceptHandler)):
            complexity += 1
        elif isinstance(child, ast.BoolOp):
            complexity += len(child.values) - 1
        elif isinstance(child, ast.comprehension):
            complexity += 1
    return complexity


def _node_span(node: ast.AST) -> int:
    end_lineno = getattr(node, 'end_lineno', None) or getattr(node, 'lineno', 1)
    lineno = getattr(node, 'lineno', 1)
    return max(1, end_lineno - lineno + 1)


def _label_path(path: Path, base_root: Path) -> str:
    try:
        return path.relative_to(base_root).as_posix()
    except ValueError:
        return path.as_posix()


def _dedupe(items: object) -> tuple[str, ...]:
    seen: set[str] = set()
    ordered: list[str] = []
    for item in items:  # type: ignore[assignment]
        normalized = str(item).strip()
        if not normalized or normalized in seen:
            continue
        seen.add(normalized)
        ordered.append(normalized)
    return tuple(ordered)


def _dedupe_paths(paths: list[Path]) -> tuple[Path, ...]:
    seen: set[Path] = set()
    ordered: list[Path] = []
    for path in paths:
        if path in seen:
            continue
        seen.add(path)
        ordered.append(path)
    return tuple(ordered)
