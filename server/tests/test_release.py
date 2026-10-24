"""Offline checks for first-run config and source-only release boundaries."""
import ast
import importlib.util
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

BASE = Path(__file__).resolve().parents[1]


class Release(unittest.TestCase):
    def test_blank_identity_and_wakeup(self):
        spec = importlib.util.spec_from_file_location("clean_config", BASE / "config.example.py")
        config = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(config)
        self.assertEqual(config.APP_CONFIG["wakeup"]["keywords"], ["你好小智"])
        for field in ("DEVICE_ID", "VERIFICATION_CODE", "WEBSOCKET_ACCESS_TOKEN"):
            self.assertEqual(config.APP_CONFIG["xiaozhi"][field], "")

    def test_initializer_preserves_existing_pairing_data(self):
        with tempfile.TemporaryDirectory() as temporary:
            target = Path(temporary) / "data/config.py"
            command = [sys.executable, str(BASE / "init_config.py"), "--path", str(target)]
            subprocess.run(command, check=True, capture_output=True)
            self.assertEqual(target.read_bytes(), (BASE / "config.example.py").read_bytes())
            target.write_text("synthetic-existing-pairing-state", encoding="utf-8")
            subprocess.run(command, check=True, capture_output=True)
            self.assertEqual(target.read_text(encoding="utf-8"), "synthetic-existing-pairing-state")

    def test_published_python_is_syntactically_valid(self):
        for path in BASE.rglob("*.py"):
            ast.parse(path.read_text(encoding="utf-8"), filename=str(path))


if __name__ == "__main__":
    unittest.main()
