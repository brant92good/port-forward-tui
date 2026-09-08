"""Build and execute the native helper without changing desktop focus."""
import base64
import json
import subprocess
import unittest
import uuid

from build_focus_helper import ensure_helper


class NativeHelperTests(unittest.TestCase):
    def test_missing_tab_probe_returns_no_match(self):
        executable = ensure_helper()
        payload = base64.b64encode(json.dumps(["Absent test tab " + uuid.uuid4().hex]).encode()).decode()
        result = subprocess.run([str(executable), "-TitlesBase64", payload, "-ProbeOnly"],
                                capture_output=True, creationflags=subprocess.CREATE_NO_WINDOW, timeout=15)
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertEqual(result.stdout, b"")
