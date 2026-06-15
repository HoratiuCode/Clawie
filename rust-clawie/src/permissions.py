from __future__ import annotations

import json
from dataclasses import dataclass, field
from datetime import datetime, timezone
from enum import Enum
from pathlib import Path
from typing import Sequence


# ---------------------------------------------------------------------------
# Permission action taxonomy
# ---------------------------------------------------------------------------

class PermissionAction(Enum):
    """Fine-grained action categories inspired by Antigravity / Gemini CLI."""

    read_file = "read_file"
    write_file = "write_file"
    command = "command"
    read_url = "read_url"
    execute_url = "execute_url"
    mcp = "mcp"
    custom = "custom"
    unsandboxed = "unsandboxed"


# ---------------------------------------------------------------------------
# Single permission grant
# ---------------------------------------------------------------------------

@dataclass(frozen=True)
class PermissionGrant:
    """An individual permission entry.

    Parameters
    ----------
    action:
        The action category this grant covers.
    target:
        A pattern describing *what* is permitted:
        - ``read_file`` / ``write_file`` – absolute path prefix
        - ``command`` / ``unsandboxed`` – command-token prefix
        - ``read_url`` / ``execute_url`` – domain (sub-domain matching)
        - ``mcp`` – ``serverName/toolName`` or ``serverName/*``
        - ``custom`` – arbitrary string (exact match)
    reason:
        Human-readable justification for the grant.
    granted_at:
        ISO-8601 timestamp of when the grant was created.
    """

    action: PermissionAction
    target: str
    reason: str = ""
    granted_at: str = field(default_factory=lambda: datetime.now(timezone.utc).isoformat())

    # -- serialisation helpers ------------------------------------------------

    def to_dict(self) -> dict[str, str]:
        return {
            "action": self.action.value,
            "target": self.target,
            "reason": self.reason,
            "granted_at": self.granted_at,
        }

    @classmethod
    def from_dict(cls, data: dict[str, str]) -> PermissionGrant:
        return cls(
            action=PermissionAction(data["action"]),
            target=data["target"],
            reason=data.get("reason", ""),
            granted_at=data.get("granted_at", ""),
        )


# ---------------------------------------------------------------------------
# Matching helpers (pure functions)
# ---------------------------------------------------------------------------

def _match_path(grant_path: str, query_path: str) -> bool:
    """Return ``True`` when *query_path* is equal to or nested inside *grant_path*.

    Both paths are normalised before comparison so trailing slashes and
    redundant separators are ignored.
    """
    gp = Path(grant_path).resolve()
    qp = Path(query_path).resolve()
    # qp is gp itself or a child of gp
    try:
        qp.relative_to(gp)
        return True
    except ValueError:
        return False


def _match_command_prefix(grant_prefix: str, query_command: str) -> bool:
    """Token-level prefix match.

    A grant for ``"git"`` matches ``"git"``, ``"git add"``, ``"git commit -m …"``
    but *not* ``"github-cli …"``.
    """
    grant_tokens = grant_prefix.split()
    query_tokens = query_command.split()
    if len(query_tokens) < len(grant_tokens):
        return False
    return query_tokens[: len(grant_tokens)] == grant_tokens


def _match_domain(grant_domain: str, query_domain: str) -> bool:
    """Domain / sub-domain matching.

    ``"google.com"`` matches ``"google.com"`` **and** ``"docs.google.com"``.
    """
    gd = grant_domain.lower().lstrip(".")
    qd = query_domain.lower()
    return qd == gd or qd.endswith("." + gd)


def _match_mcp(grant_pattern: str, query_target: str) -> bool:
    """MCP server/tool matching.

    ``"myserver/*"`` matches any tool on *myserver*.
    ``"myserver/myTool"`` matches only that specific tool.
    """
    gp = grant_pattern.strip()
    qt = query_target.strip()

    if "/" not in gp:
        # Bare server name → treat as server/*
        return qt == gp or qt.startswith(gp + "/")

    g_server, g_tool = gp.split("/", 1)
    if "/" not in qt:
        return False
    q_server, q_tool = qt.split("/", 1)

    if g_server != q_server:
        return False
    return g_tool == "*" or g_tool == q_tool


# Map actions → matchers
_MATCHERS: dict[PermissionAction, object] = {
    PermissionAction.read_file: _match_path,
    PermissionAction.write_file: _match_path,
    PermissionAction.command: _match_command_prefix,
    PermissionAction.unsandboxed: _match_command_prefix,
    PermissionAction.read_url: _match_domain,
    PermissionAction.execute_url: _match_domain,
    PermissionAction.mcp: _match_mcp,
}


# ---------------------------------------------------------------------------
# PermissionStore
# ---------------------------------------------------------------------------

class PermissionStore:
    """Manages a collection of :class:`PermissionGrant` entries with
    category-aware matching logic and JSON persistence.
    """

    def __init__(self, grants: Sequence[PermissionGrant] | None = None) -> None:
        self._grants: list[PermissionGrant] = list(grants or [])

    # -- mutators -------------------------------------------------------------

    def grant(self, action: PermissionAction, target: str, reason: str = "") -> PermissionGrant:
        """Add a new permission grant and return it."""
        entry = PermissionGrant(action=action, target=target, reason=reason)
        self._grants.append(entry)
        return entry

    def revoke(self, action: PermissionAction, target: str) -> int:
        """Remove **all** grants matching *action* and *target* exactly.

        Returns the number of grants removed.
        """
        before = len(self._grants)
        self._grants = [
            g for g in self._grants
            if not (g.action == action and g.target == target)
        ]
        return before - len(self._grants)

    # -- query ----------------------------------------------------------------

    def check(self, action: PermissionAction, target: str) -> bool:
        """Return ``True`` if any stored grant covers *action* + *target*.

        Special rule: a ``write_file`` grant implicitly covers ``read_file``
        for the same path hierarchy.
        """
        actions_to_try: list[PermissionAction] = [action]
        if action == PermissionAction.read_file:
            actions_to_try.append(PermissionAction.write_file)

        for try_action in actions_to_try:
            matcher = _MATCHERS.get(try_action)
            for g in self._grants:
                if g.action != try_action:
                    continue
                if matcher is not None:
                    if matcher(g.target, target):  # type: ignore[operator]
                        return True
                else:
                    # custom / fallback → exact match
                    if g.target == target:
                        return True
        return False

    def list_grants(self) -> list[PermissionGrant]:
        """Return a shallow copy of all current grants."""
        return list(self._grants)

    # -- display --------------------------------------------------------------

    def as_markdown(self) -> str:
        """Render grants as a Markdown table."""
        if not self._grants:
            return "_No permission grants._"

        lines: list[str] = [
            "| Action | Target | Reason | Granted At |",
            "|--------|--------|--------|------------|",
        ]
        for g in self._grants:
            lines.append(
                f"| `{g.action.value}` | `{g.target}` | {g.reason} | {g.granted_at} |"
            )
        return "\n".join(lines)

    # -- persistence ----------------------------------------------------------

    def save(self, path: str | Path) -> None:
        """Serialise the store to a JSON file at *path*."""
        data = [g.to_dict() for g in self._grants]
        target = Path(path)
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(json.dumps(data, indent=2), encoding="utf-8")

    @classmethod
    def load(cls, path: str | Path) -> PermissionStore:
        """Deserialise a store from a JSON file at *path*."""
        target = Path(path)
        if not target.exists():
            return cls()
        raw = json.loads(target.read_text(encoding="utf-8"))
        grants = [PermissionGrant.from_dict(entry) for entry in raw]
        return cls(grants=grants)

    # -- dunder ---------------------------------------------------------------

    def __len__(self) -> int:
        return len(self._grants)

    def __bool__(self) -> bool:
        return bool(self._grants)

    def __repr__(self) -> str:
        return f"PermissionStore(grants={len(self._grants)})"


# ---------------------------------------------------------------------------
# Backward-compatible ToolPermissionContext
# ---------------------------------------------------------------------------

@dataclass(frozen=True)
class ToolPermissionContext:
    """Legacy deny-list based permission context.

    Maintained for backward compatibility.  Internally it can optionally
    delegate richer checks to a :class:`PermissionStore`.
    """

    deny_names: frozenset[str] = field(default_factory=frozenset)
    deny_prefixes: tuple[str, ...] = ()
    _store: PermissionStore | None = field(default=None, repr=False, compare=False)

    # -- legacy constructors --------------------------------------------------

    @classmethod
    def from_iterables(
        cls,
        deny_names: list[str] | None = None,
        deny_prefixes: list[str] | None = None,
    ) -> ToolPermissionContext:
        return cls(
            deny_names=frozenset(name.lower() for name in (deny_names or [])),
            deny_prefixes=tuple(prefix.lower() for prefix in (deny_prefixes or [])),
        )

    @classmethod
    def from_permission_store(cls, store: PermissionStore) -> ToolPermissionContext:
        """Create a context backed by a :class:`PermissionStore`.

        The deny-list fields are left empty; all checking is forwarded to the
        store's :meth:`~PermissionStore.check` method via :meth:`blocks`.
        """
        return cls(_store=store)

    # -- query ----------------------------------------------------------------

    def blocks(self, tool_name: str) -> bool:
        """Return ``True`` if *tool_name* is blocked by the deny-list rules."""
        lowered = tool_name.lower()
        if lowered in self.deny_names:
            return True
        if any(lowered.startswith(prefix) for prefix in self.deny_prefixes):
            return True
        return False

    @property
    def store(self) -> PermissionStore | None:
        """Access the underlying :class:`PermissionStore`, if any."""
        return self._store
