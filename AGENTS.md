# Working on Port Forward TUI

This Windows app lets someone open a remote web app through an address on their
own computer. A favorite stores a name and two port numbers. SSH carries the
connection; a background process owns it so closing the screen does not stop it.

## First actions

- Read README.md for setup and command examples. Run `./doctor.ps1 --json` for
  local prerequisites. It does not install, write settings, or contact SSH.
- Use `./ports.ps1 list --json` to inspect favorites. Use the documented save,
  start, stop and delete commands rather than screen automation or editing live
  JSON. Read IDs from results. `UNKNOWN` means no live status was observed;
  `ON` confirms a local listener, not a working remote web service.
- Installation: `./install.ps1 -HostName EXISTING_ALIAS -NonInteractive`.
  Get the SSH name from the user or existing configuration. Do not invent one.
  Honor the user's existing authorization for installation, tests and publishing.
- JSON has `schema_version: 1` and `ok`. Exit codes: 0 success, 1 failed check or
  operation, 2 invalid arguments. Startup without usable Python is reported by
  PowerShell before the JSON command can run.

## Code map and invariants

- `app.py` / `app.tcss`: screen, keyboard actions, add/edit forms, help.
- `ports.py` / `diagnostics.py`: headless commands and local checks.
- `forwarding.py`: file schema, SSH arguments, owned Windows process jobs.
- `background.py`: authenticated local control server and serialized writes.
- `launch.py`, `views.py`, `FocusHelper.cs`: lightweight return shortcut.
- `install.ps1`, `python_bootstrap.ps1`: private environment and Terminal entry.

Keep the existing-view return path free of Textual imports. Preserve concurrent
favorite edits through the server. Never infer an SSH connection from a UI row
alone or claim persistence across reboot. Don't kill unrelated processes, use
real favorites in tests, or publish endpoint.json, SSH keys, hosts or private
paths. Use isolated `--data-dir` folders and explicit cleanup.

## Verification and shipping

Run `.\.venv\Scripts\python.exe -E -s -m unittest discover -v`. Tests cover real
local control-server requests and keyboard flows without needing a remote host.
`check_live.py --host ALIAS` is an opt-in real SSH check. Desktop focus tests
live in the companion terminal-workspace repository and move real windows.
Screenshots: `scripts/capture_screenshots.py` renders the actual UI with clearly
labeled simulated data. Review images before publishing.

This public repository contains no personal setup. When it is included as a
submodule (a repository pinned inside another), publish this commit first, then
update the parent's recorded commit. Keep examples generic and explain new
terms before asking a beginner to make a choice.
