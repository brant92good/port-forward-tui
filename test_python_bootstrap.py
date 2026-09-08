"""Exercise setup in real Windows processes without modifying global Python."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parent
POWERSHELL = Path(os.environ.get("SystemRoot", r"C:\Windows")) / "System32/WindowsPowerShell/v1.0/powershell.exe"


class PythonBootstrapTests(unittest.TestCase):
    def run_script(self, folder, script, *arguments, env=None):
        path = Path(folder) / "probe.ps1"
        path.write_text(script, encoding="utf-8-sig")
        return subprocess.run([str(POWERSHELL), "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", str(path),
                               *map(str, arguments)], capture_output=True, text=True, errors="replace",
                              env=env, creationflags=subprocess.CREATE_NO_WINDOW, timeout=45)

    def test_creates_isolated_environment_in_unicode_path_with_poisoned_python_variables(self):
        with tempfile.TemporaryDirectory() as folder:
            target = Path(folder) / "App workspace \u6e2c\u8a66 O'Connor"
            target.mkdir()
            script = """param([string]$Bootstrap,[string]$Root,[string]$Python)
. $Bootstrap
$installedPython = Initialize-AppPython -Root $Root -Python $Python
$info = Get-AppPythonInfo -Executable $installedPython
$info | ConvertTo-Json -Compress
"""
            # param must precede executable statements in a PowerShell script.
            script = script.replace(". $Bootstrap", "$ErrorActionPreference = 'Stop'\n. $Bootstrap")
            path = Path(folder) / "bootstrap-check.ps1"
            path.write_text(script, encoding="utf-8-sig")
            env = dict(os.environ, PYTHONHOME=str(Path(folder) / "wrong-home"), PYTHONPATH=str(Path(folder) / "unrelated-project"))
            env["PATH"] = os.pathsep.join([str(Path(os.environ["SystemRoot"]) / "System32"), os.environ["SystemRoot"]])
            result = subprocess.run([str(POWERSHELL), "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", str(path),
                                     str(ROOT / "python_bootstrap.ps1"), str(target), sys._base_executable],
                                    env=env, capture_output=True, text=True, errors="replace",
                                    creationflags=subprocess.CREATE_NO_WINDOW, timeout=45)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            info = json.loads(result.stdout.splitlines()[-1])
            self.assertEqual(Path(info["prefix"]), target / ".venv")
            self.assertNotEqual(info["prefix"], info["base_prefix"])
            checked = subprocess.run([str(target / ".venv/Scripts/python.exe"), "-E", "-s", str(ROOT / "app.py"),
                                      "--check", "--host", "workbox", "--data-dir", str(Path(folder) / "data")],
                                     env=env, capture_output=True, text=True, creationflags=subprocess.CREATE_NO_WINDOW, timeout=10)
            self.assertEqual(checked.returncode, 0, checked.stderr)

    def test_incomplete_environment_is_preserved(self):
        with tempfile.TemporaryDirectory() as folder:
            target = Path(folder) / ".venv"
            target.mkdir()
            marker = target / "keep.txt"
            marker.write_text("user data")
            script = "param([string]$Bootstrap,[string]$Root)\n$ErrorActionPreference = 'Stop'\n. $Bootstrap\nInitialize-AppPython -Root $Root\n"
            result = self.run_script(folder, script, ROOT / "python_bootstrap.ps1", folder)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("incomplete", result.stderr)
            self.assertEqual(marker.read_text(), "user data")

    def test_explicit_missing_python_reports_actionable_error(self):
        with tempfile.TemporaryDirectory() as folder:
            script = "param([string]$Bootstrap,[string]$Python)\n$ErrorActionPreference = 'Stop'\n. $Bootstrap\nResolve-AppPython -Python $Python\n"
            result = self.run_script(folder, script, ROOT / "python_bootstrap.ps1", Path(folder) / "missing.exe")
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("No usable Windows Python 3.12+", result.stderr)
            self.assertIn("-Python", result.stderr)

    def test_auto_discovery_finds_supported_windows_python(self):
        with tempfile.TemporaryDirectory() as folder:
            script = "param([string]$Bootstrap)\n$ErrorActionPreference = 'Stop'\n. $Bootstrap\nResolve-AppPython | ConvertTo-Json -Compress\n"
            result = self.run_script(folder, script, ROOT / "python_bootstrap.ps1")
            self.assertEqual(result.returncode, 0, result.stderr)
            info = json.loads(result.stdout)
            self.assertEqual(info["platform"], "win32")
            self.assertGreaterEqual(info["version"][:2], [3, 12])
