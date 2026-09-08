import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parent
POWERSHELL = Path(os.environ['SystemRoot']) / 'System32/WindowsPowerShell/v1.0/powershell.exe'


class GuidedSetupTests(unittest.TestCase):
    def test_noninteractive_setup_reuses_saved_host_and_fails_when_missing(self):
        with tempfile.TemporaryDirectory() as folder:
            script = Path(folder) / 'check.ps1'
            script.write_text("""param([string]$Helpers,[string]$Settings)
$ErrorActionPreference = 'Stop'
. $Helpers
Get-SetupHost -SettingsPath $Settings -NonInteractive
""", encoding='utf-8-sig')
            settings = Path(folder) / 'settings.json'
            args = [str(POWERSHELL), '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', str(script),
                    str(ROOT / 'setup_helpers.ps1'), str(settings)]
            missing = subprocess.run(args, capture_output=True, text=True, creationflags=subprocess.CREATE_NO_WINDOW, timeout=8)
            self.assertNotEqual(missing.returncode, 0)
            self.assertIn('remote computer is required', missing.stderr)
            self.assertFalse(settings.exists())
            settings.write_text(json.dumps({'host': 'workbox'}))
            saved = subprocess.run(args, capture_output=True, text=True, creationflags=subprocess.CREATE_NO_WINDOW, timeout=8)
            self.assertEqual(saved.returncode, 0, saved.stderr)
            self.assertEqual(saved.stdout.strip(), 'workbox')
