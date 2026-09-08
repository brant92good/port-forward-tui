"""Shortcut preferences, separate from SSH favorites and tunnel state."""
import json
import os
from pathlib import Path
import tempfile

SCOPES = ("all", "window")
SCOPE_LABELS = ("All Terminal windows", "Current Terminal window only")


def read_scope(directory: Path) -> str:
    path = Path(directory) / "ui-settings.json"
    if not path.exists():
        return "all"
    value = json.loads(path.read_text(encoding="utf-8")).get("focus_scope", "all")
    if value not in SCOPES:
        raise ValueError("focus_scope must be 'all' or 'window' in ui-settings.json")
    return value


def save_scope(directory: Path, scope: str):
    if scope not in SCOPES:
        raise ValueError("Unknown focus scope")
    directory = Path(directory)
    directory.mkdir(parents=True, exist_ok=True)
    path = directory / "ui-settings.json"
    settings = json.loads(path.read_text(encoding="utf-8")) if path.exists() else {}
    settings["focus_scope"] = scope
    descriptor, temporary = tempfile.mkstemp(prefix="ui-settings-", suffix=".tmp", dir=directory)
    try:
        with os.fdopen(descriptor, "w", encoding="utf-8") as output:
            json.dump(settings, output, indent=2)
            output.write("\n")
        os.replace(temporary, path)
    finally:
        Path(temporary).unlink(missing_ok=True)
