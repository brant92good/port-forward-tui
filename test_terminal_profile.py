import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


class TerminalRegistrationTests(unittest.TestCase):
    def test_registration_preserves_settings_and_is_idempotent(self):
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / "settings.json"
            original = {"defaultProfile": "existing", "profiles": {"defaults": {"font": {"size": 15}},
                        "list": [{"guid": "existing", "name": "My Shell", "hidden": False}]},
                        "keybindings": [{"id": "custom", "keys": "ctrl+alt+p"}]}
            path.write_text(json.dumps(original), encoding="utf-8")
            command = [sys.executable, str(Path(__file__).with_name("terminal_profile.py")),
                       "--settings", str(path), "--name", "Ports test"]
            for _ in range(2):
                result = subprocess.run(command, capture_output=True, text=True, timeout=10)
                self.assertEqual(result.returncode, 0, result.stderr)
            updated = json.loads(path.read_text())
            profile = updated["profiles"]["list"].pop()
            self.assertEqual(profile["name"], "Ports test")
            self.assertIn("app.py", profile["commandline"])
            self.assertEqual(updated, original)
            self.assertTrue(list(Path(folder).glob("*.bak")))

    def test_commented_settings_are_preserved(self):
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / "settings.json"
            original = '{// hand-written comment\n"profiles":{"list":[]}}'
            path.write_text(original)
            result = subprocess.run([sys.executable, str(Path(__file__).with_name("terminal_profile.py")),
                                     "--settings", str(path)], capture_output=True, text=True, timeout=10)
            self.assertNotEqual(result.returncode, 0)
            self.assertEqual(path.read_text(), original)


if __name__ == "__main__":
    unittest.main()
