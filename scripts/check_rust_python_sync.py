#!/usr/bin/env python3
"""
Clawie Rust-Python Sync Auditor.
Compares the active Rust runtime codebase (rust-clawie/rust) against the Python mirror (python-clawie).
Reports sync drift for commands, tools, and mirrored source files.
"""

import json
import re
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
RUST_CLAWIE_ROOT = REPO_ROOT / 'rust-clawie'
PYTHON_CLAWIE_ROOT = REPO_ROOT / 'python-clawie'

def check_commands():
    rust_commands_path = RUST_CLAWIE_ROOT / 'rust' / 'crates' / 'commands' / 'commands.json'
    python_commands_path = PYTHON_CLAWIE_ROOT / 'src' / 'reference_data' / 'commands_snapshot.json'
    
    if not rust_commands_path.exists() or not python_commands_path.exists():
        return [], [], "Missing command files"
        
    try:
        rust_commands = json.loads(rust_commands_path.read_text(encoding='utf-8'))
        python_commands = json.loads(python_commands_path.read_text(encoding='utf-8'))
    except Exception as e:
        return [], [], f"Failed to parse JSON: {e}"
        
    rust_names = {cmd['name'] for cmd in rust_commands}
    python_names = {cmd['name'] for cmd in python_commands}
    
    missing_in_rust = sorted(list(python_names - rust_names))
    missing_in_python = sorted(list(rust_names - python_names))
    return missing_in_rust, missing_in_python, None

def check_tools():
    rust_tools_path = RUST_CLAWIE_ROOT / 'rust' / 'crates' / 'tools' / 'src' / 'lib.rs'
    python_tools_path = PYTHON_CLAWIE_ROOT / 'src' / 'reference_data' / 'tools_snapshot.json'
    
    if not rust_tools_path.exists() or not python_tools_path.exists():
        return [], [], "Missing tool files"
        
    try:
        rust_content = rust_tools_path.read_text(encoding='utf-8')
        rust_tool_names = set(re.findall(r'name:\s*"([^"]+)"', rust_content))
        python_tools = json.loads(python_tools_path.read_text(encoding='utf-8'))
    except Exception as e:
        return [], [], f"Failed to parse tool data: {e}"
        
    python_tool_names = {tool['name'] for tool in python_tools}
    
    missing_in_rust = sorted(list(python_tool_names - rust_tool_names))
    missing_in_python = sorted(list(rust_tool_names - python_tool_names))
    return missing_in_rust, missing_in_python, None

def check_src_files():
    rust_src = RUST_CLAWIE_ROOT / 'src'
    python_src = PYTHON_CLAWIE_ROOT / 'src'
    
    if not rust_src.exists() or not python_src.exists():
        return [], [], "Missing source directories"
        
    rust_files = {p.relative_to(rust_src) for p in rust_src.rglob('*.py') if '__pycache__' not in p.parts}
    python_files = {p.relative_to(python_src) for p in python_src.rglob('*.py') if '__pycache__' not in p.parts}
    
    missing_in_python = sorted([str(f) for f in (rust_files - python_files)])
    missing_in_rust = sorted([str(f) for f in (python_files - rust_files)])
    
    # Check content diffs for common files
    common_files = rust_files.intersection(python_files)
    content_diffs = []
    for f in sorted(list(common_files)):
        r_content = (rust_src / f).read_text(encoding='utf-8')
        p_content = (python_src / f).read_text(encoding='utf-8')
        if r_content != p_content:
            content_diffs.append(str(f))
            
    return missing_in_rust, missing_in_python, content_diffs, None

def main():
    print("# Clawie Rust-Python Sync Audit Report")
    print(f"Analyzing workspace roots:\n- Rust Runtime: `{RUST_CLAWIE_ROOT.relative_to(REPO_ROOT)}`\n- Python Mirror: `{PYTHON_CLAWIE_ROOT.relative_to(REPO_ROOT)}`")
    print()
    
    # 1. Command definitions
    cmd_rust_missing, cmd_py_missing, cmd_err = check_commands()
    print("## 1. Command Parity")
    if cmd_err:
        print(f"⚠️ Error: {cmd_err}")
    else:
        print(f"- Command name drift: {len(cmd_rust_missing) + len(cmd_py_missing)}")
        print(f"- Commands missing in Python mirror: **{len(cmd_py_missing)}**")
        if cmd_py_missing:
            print("  *(Newer Rust commands not yet mirrored)*")
            for c in cmd_py_missing[:10]:
                print(f"  - `{c}`")
            if len(cmd_py_missing) > 10:
                print(f"  - ... and {len(cmd_py_missing) - 10} more")
        print(f"- Obsolete commands in Python snapshot (not in Rust): **{len(cmd_rust_missing)}**")
        if cmd_rust_missing:
            for c in cmd_rust_missing[:10]:
                print(f"  - `{c}`")
            if len(cmd_rust_missing) > 10:
                print(f"  - ... and {len(cmd_rust_missing) - 10} more")
    print()

    # 2. Tool definitions
    tool_rust_missing, tool_py_missing, tool_err = check_tools()
    print("## 2. Tool Parity")
    if tool_err:
        print(f"⚠️ Error: {tool_err}")
    else:
        print(f"- Tools missing in Python mirror: **{len(tool_py_missing)}**")
        if tool_py_missing:
            for t in tool_py_missing[:10]:
                print(f"  - `{t}`")
            if len(tool_py_missing) > 10:
                print(f"  - ... and {len(tool_py_missing) - 10} more")
        print(f"- Obsolete tools in Python snapshot (not in Rust): **{len(tool_rust_missing)}**")
        if tool_rust_missing:
            for t in tool_rust_missing[:10]:
                print(f"  - `{t}`")
            if len(tool_rust_missing) > 10:
                print(f"  - ... and {len(tool_rust_missing) - 10} more")
    print()

    # 3. Source File Sync
    file_rust_missing, file_py_missing, content_diffs, file_err = check_src_files()
    print("## 3. Source Code Sync")
    if file_err:
        print(f"⚠️ Error: {file_err}")
    else:
        print(f"- Python files missing in Python mirror: **{len(file_py_missing)}**")
        if file_py_missing:
            for f in file_py_missing:
                print(f"  - `{f}`")
        print(f"- Python files missing in Rust runtime copy: **{len(file_rust_missing)}**")
        if file_rust_missing:
            for f in file_rust_missing:
                print(f"  - `{f}`")
        print(f"- Mirrored files with content differences: **{len(content_diffs)}**")
        if content_diffs:
            for f in content_diffs[:15]:
                print(f"  - `{f}`")
            if len(content_diffs) > 15:
                print(f"  - ... and {len(content_diffs) - 15} more")

    # Exit code indicates if there is any critical drift (excluding historical snapshot discrepancies)
    # We exit with 0 if content diffs and missing files are clean.
    has_critical_drift = len(file_py_missing) > 0 or len(file_rust_missing) > 0 or len(content_diffs) > 0
    if has_critical_drift:
        print("\n❌ Sync Check Failed: Core code assets are out of sync!")
        sys.exit(1)
    else:
        print("\n✅ Sync Check Passed: Mirrored codebase files are perfectly in sync.")
        sys.exit(0)

if __name__ == '__main__':
    main()
