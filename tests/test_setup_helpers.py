import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
POWERSHELL = Path(os.environ['SystemRoot']) / 'System32/WindowsPowerShell/v1.0/powershell.exe'


class GuidedSetupTests(unittest.TestCase):
    def test_noninteractive_setup_allows_no_host_and_reuses_existing_optional_host(self):
        with tempfile.TemporaryDirectory() as folder:
            script = Path(folder) / 'check.ps1'
            script.write_text("""param([string]$Helpers,[string]$Settings)
$ErrorActionPreference = 'Stop'
. $Helpers
Get-SetupHost -SettingsPath $Settings -NonInteractive
""", encoding='utf-8-sig')
            settings = Path(folder) / 'settings.json'
            args = [str(POWERSHELL), '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', str(script),
                    str(ROOT / 'scripts/setup_helpers.ps1'), str(settings)]
            missing = subprocess.run(args, capture_output=True, text=True, creationflags=subprocess.CREATE_NO_WINDOW, timeout=8)
            self.assertEqual(missing.returncode, 0, missing.stderr)
            self.assertEqual(missing.stdout.strip(), '')
            self.assertFalse(settings.exists())
            settings.write_text(json.dumps({'host': 'workbox'}))
            saved = subprocess.run(args, capture_output=True, text=True, creationflags=subprocess.CREATE_NO_WINDOW, timeout=8)
            self.assertEqual(saved.returncode, 0, saved.stderr)
            self.assertEqual(saved.stdout.strip(), 'workbox')
