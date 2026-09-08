"""Switching an existing view must not start the TUI framework."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


class LightweightLaunchTests(unittest.TestCase):
    def test_existing_view_switch_does_not_import_the_ui(self):
        code = '''import sys
from pathlib import Path
from unittest.mock import patch
from forwarding import Store
store = Store(Path(sys.argv[1]))
store.host = "workbox"
store.save([])
sys.argv = ["app.py", "--data-dir", str(store.directory), "--focus-existing"]
with patch("views.focus_existing", return_value=True) as focus:
    from launch import main
    assert main() == 0
    focus.assert_called_once_with(store.directory)
assert "app" not in sys.modules
assert "textual" not in sys.modules
'''
        with tempfile.TemporaryDirectory() as directory:
            result = subprocess.run([sys.executable, "-c", code, directory], capture_output=True, text=True,
                                    creationflags=subprocess.CREATE_NO_WINDOW, timeout=10)
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_script_check_uses_lightweight_cli(self):
        with tempfile.TemporaryDirectory() as directory:
            result = subprocess.run([sys.executable, str(Path(__file__).with_name("app.py")),
                                     "--data-dir", directory, "--host", "workbox", "--check"],
                                    capture_output=True, text=True, creationflags=subprocess.CREATE_NO_WINDOW, timeout=10)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("6 saved forwards", result.stdout)
