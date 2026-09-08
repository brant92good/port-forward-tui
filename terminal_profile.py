"""Register one Windows Terminal profile without changing other profiles or defaults."""
import argparse
from datetime import datetime
import json
import os
from pathlib import Path
import subprocess
import sys
import uuid


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--name", default="Port Forward TUI")
    parser.add_argument("--settings", type=Path, help="Explicit Windows Terminal settings.json path")
    parser.add_argument("--focus-shortcut", help="Optional Terminal key chord that focuses an existing view or opens one")
    options = parser.parse_args()
    local = Path(os.environ["LOCALAPPDATA"])
    candidates = [local / "Packages/Microsoft.WindowsTerminal_8wekyb3d8bbwe/LocalState/settings.json",
                  local / "Microsoft/Windows Terminal/settings.json",
                  local / "Packages/Microsoft.WindowsTerminalPreview_8wekyb3d8bbwe/LocalState/settings.json"]
    path = options.settings or next((p for p in candidates if p.exists()), None)
    if path is None:
        raise SystemExit("Open Windows Terminal once to create its settings, then retry.")
    original = path.read_bytes()
    try:
        data = json.loads(original)
    except ValueError:
        raise SystemExit("Settings contain JSON comments or invalid JSON. Add a Terminal profile manually using the README command; the file was not changed.")
    root = Path(__file__).resolve().parent
    python = root / ".venv/Scripts/python.exe"
    if not python.exists():
        python = Path(sys.executable)
    guid = "{" + str(uuid.uuid5(uuid.NAMESPACE_URL, f"port-forward-tui:{root}")) + "}"
    profiles = data.setdefault("profiles", {}).setdefault("list", [])
    found = next((p for p in profiles if p.get("guid") == guid), None)
    profile = {"guid": guid, "name": options.name,
               "commandline": subprocess.list2cmdline([str(python), "-E", "-s", str(root / "app.py")]),
               "startingDirectory": "%USERPROFILE%", "icon": "\U0001f50c",
               "hidden": False, "closeOnExit": "automatic"}
    if found is None:
        profiles.append(profile)
    else:
        found.update(profile)
    if options.focus_shortcut:
        action_id = "User.PortForwardTUI.Focus." + guid.strip("{}")
        for binding in data.get("keybindings", []):
            if binding.get("keys") == options.focus_shortcut and binding.get("id") != action_id:
                raise SystemExit("That shortcut is already assigned. Settings were not changed.")
        command = subprocess.list2cmdline([str(python), "-E", "-s", str(root / "app.py"), "--focus-existing"])
        action = {"id": action_id, "command": {"action": "newTab", "profile": guid, "commandline": command}}
        actions = data.setdefault("actions", [])
        previous = next((a for a in actions if a.get("id") == action_id), None)
        if previous:
            previous.update(action)
        else:
            actions.append(action)
        keys = data.setdefault("keybindings", [])
        if not any(k.get("id") == action_id and k.get("keys") == options.focus_shortcut for k in keys):
            keys.append({"id": action_id, "keys": options.focus_shortcut})
    if path.read_bytes() != original:
        raise SystemExit("Terminal settings changed while preparing the profile. Please retry.")
    backup = path.with_name(path.name + ".before-port-forward-tui-" + datetime.now().strftime("%Y%m%d-%H%M%S") + ".bak")
    backup.write_bytes(original)
    temporary = path.with_name(path.name + ".ports.tmp")
    temporary.write_text(json.dumps(data, indent=4) + "\n", encoding="utf-8")
    os.replace(temporary, path)
    print(f"Added {options.name}. Existing profiles and default shell are preserved. Backup: {backup}")


if __name__ == "__main__":
    main()
