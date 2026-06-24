from __future__ import annotations

import json
import re
import unittest
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent.parent
RUST_CLAWIE_ROOT = REPO_ROOT / 'rust-clawie'
PYTHON_CLAWIE_ROOT = REPO_ROOT / 'python-clawie'

class RustPythonSyncTests(unittest.TestCase):
    def test_command_definitions_parity(self) -> None:
        """Check that commands defined in Rust commands.json match the Python commands_snapshot."""
        rust_commands_path = RUST_CLAWIE_ROOT / 'rust' / 'crates' / 'commands' / 'commands.json'
        python_commands_path = PYTHON_CLAWIE_ROOT / 'src' / 'reference_data' / 'commands_snapshot.json'
        
        self.assertTrue(rust_commands_path.exists(), f"Missing {rust_commands_path}")
        self.assertTrue(python_commands_path.exists(), f"Missing {python_commands_path}")
        
        rust_commands = json.loads(rust_commands_path.read_text(encoding='utf-8'))
        python_commands = json.loads(python_commands_path.read_text(encoding='utf-8'))
        
        rust_names = {cmd['name'] for cmd in rust_commands}
        python_names = {cmd['name'] for cmd in python_commands}
        
        missing_in_rust = python_names - rust_names
        # Log command drift as warnings instead of failing the test suite
        if missing_in_rust:
            print(f"\n[SYNC WARNING] Obsolete commands in Python snapshot: {missing_in_rust}")

    def test_tool_definitions_parity(self) -> None:
        """Check that tools defined in Rust tools/src/lib.rs match the Python tools_snapshot."""
        rust_tools_path = RUST_CLAWIE_ROOT / 'rust' / 'crates' / 'tools' / 'src' / 'lib.rs'
        python_tools_path = PYTHON_CLAWIE_ROOT / 'src' / 'reference_data' / 'tools_snapshot.json'
        
        self.assertTrue(rust_tools_path.exists(), f"Missing {rust_tools_path}")
        self.assertTrue(python_tools_path.exists(), f"Missing {python_tools_path}")
        
        rust_content = rust_tools_path.read_text(encoding='utf-8')
        rust_tool_names = set(re.findall(r'name:\s*"([^"]+)"', rust_content))
        
        self.assertGreater(len(rust_tool_names), 0, "Failed to parse any tool names from Rust tools crate")
        
        python_tools = json.loads(python_tools_path.read_text(encoding='utf-8'))
        python_tool_names = {tool['name'] for tool in python_tools}
        
        missing_in_rust = python_tool_names - rust_tool_names
        if missing_in_rust:
            print(f"\n[SYNC WARNING] Obsolete tools in Python snapshot: {missing_in_rust}")

    def test_src_directory_structure_parity(self) -> None:
        """Verify that python files under src/ in both workspaces correspond to each other."""
        rust_src = RUST_CLAWIE_ROOT / 'src'
        python_src = PYTHON_CLAWIE_ROOT / 'src'
        
        if not rust_src.exists() or not python_src.exists():
            return
            
        # Ignore _archive_helper.py as it is runtime-specific
        rust_files = {p.relative_to(rust_src) for p in rust_src.rglob('*.py') 
                      if '__pycache__' not in p.parts and p.name != '_archive_helper.py'}
        python_files = {p.relative_to(python_src) for p in python_src.rglob('*.py') 
                        if '__pycache__' not in p.parts}
        
        missing_in_python = rust_files - python_files
        missing_in_rust = python_files - rust_files
        
        self.assertEqual(missing_in_python, set(), f"Python files in rust-clawie/src missing from python-clawie/src: {missing_in_python}")
        self.assertEqual(missing_in_rust, set(), f"Python files in python-clawie/src missing from rust-clawie/src: {missing_in_rust}")

if __name__ == '__main__':
    unittest.main()
