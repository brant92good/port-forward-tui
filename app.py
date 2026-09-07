"""Keyboard-first local port forwarding for Windows Terminal."""
from __future__ import annotations

import argparse
from dataclasses import replace
from pathlib import Path
import sys
import webbrowser

from rich.text import Text
from textual import events, on
from textual.app import App, ComposeResult
from textual.binding import Binding
from textual.containers import Horizontal, Vertical, VerticalScroll
from textual.screen import ModalScreen
from textual.widgets import Button, DataTable, Footer, Header, Input, Label, Static

from forwarding import DATA_DIR, Forward, InstanceLock, Store, TunnelManager, port, quick_ports, validate_host


class Confirm(ModalScreen[bool]):
    BINDINGS = [Binding("escape,n", "cancel", "Cancel"), Binding("y", "yes", "Confirm")]

    def __init__(self, message: str):
        super().__init__()
        self.message = message

    def compose(self) -> ComposeResult:
        with Vertical(id="confirm-dialog"):
            yield Static(self.message, markup=False)
            with Horizontal(classes="buttons"):
                yield Button("Yes [Y]", variant="error", id="yes")
                yield Button("Cancel [Esc]", id="cancel")

    def on_mount(self):
        self.query_one("#cancel", Button).focus()

    def action_cancel(self):
        self.dismiss(False)

    def action_yes(self):
        self.dismiss(True)

    @on(Button.Pressed)
    def button(self, event: Button.Pressed):
        self.dismiss(event.button.id == "yes")


class EditForward(ModalScreen[Forward | None]):
    BINDINGS = [Binding("escape", "cancel", "Cancel"), Binding("ctrl+s", "save", "Save", priority=True)]

    def __init__(self, rule: Forward):
        super().__init__()
        self.rule = rule

    def compose(self) -> ComposeResult:
        with VerticalScroll(id="edit-dialog"):
            yield Label("EDIT SAVED FORWARD", classes="dialog-title")
            yield Label("Name")
            yield Input(self.rule.name, id="name", max_length=80)
            yield Label("Remote port on the SSH host")
            yield Input(str(self.rule.remote_port), id="remote", max_length=5)
            yield Label("Local port on this computer (blank = same as remote)")
            yield Input(str(self.rule.local_port), id="local", max_length=5)
            yield Static("Enter or Ctrl+S saves. An active tunnel restarts with your changes.", classes="muted")
            yield Static("", id="edit-error", markup=False)
            with Horizontal(classes="buttons"):
                yield Button("Save", variant="primary", id="save")
                yield Button("Cancel", id="cancel")

    def on_mount(self):
        self.query_one("#local", Input).focus()
        self.query_one("#local", Input).action_select_all()

    def action_cancel(self):
        self.dismiss(None)

    def action_save(self):
        try:
            remote = port(self.query_one("#remote", Input).value.strip())
            local_text = self.query_one("#local", Input).value.strip()
            local = port(local_text) if local_text else remote
            name = self.query_one("#name", Input).value.strip() or f"Port {remote}"
            self.dismiss(replace(self.rule, name=name, local_port=local, remote_port=remote))
        except ValueError as error:
            self.query_one("#edit-error", Static).update(str(error))

    @on(Input.Submitted)
    def submitted(self):
        self.action_save()

    @on(Button.Pressed)
    def button(self, event: Button.Pressed):
        self.action_save() if event.button.id == "save" else self.action_cancel()


class Help(ModalScreen):
    BINDINGS = [Binding("escape,question_mark,q", "close", "Close")]

    def compose(self) -> ComposeResult:
        with VerticalScroll(id="help-dialog"):
            yield Static("""[bold cyan]PORT FORWARD TUI[/]

[bold]Quick forward[/]
Just type a number, then Enter. It is saved and started.
  8000          local 8000 -> remote 8000
  18000:8000    local 18000 -> remote 8000
  8888 Jupyter  optional friendly name after a space

[bold]Keyboard[/]
Up / Down     Select a saved forward
Enter / Space Start or stop the selected forward
N             Focus the quick-forward box
E             Edit name and ports (local port is selected)
D             Delete a saved forward
R             Restart the selected forward
B             Open http://127.0.0.1:LOCAL in your browser
S             Stop all tunnels
Esc           Return from quick entry to the saved list
Q / Ctrl+Q    Close the UI (background tunnels keep running)
?             This help

Favorites are saved automatically. Nothing starts automatically.
Background mode is ON by default: you may close the entire Terminal app.
Reopen the UI to manage the same running tunnels. S explicitly stops all.
With --foreground, closing the UI stops its tunnels instead.
Signing out, rebooting, or loss of the SSH connection ends the tunnels.
ON means SSH is listening locally; the remote service must be running.
Only 127.0.0.1 on this computer can access these forwards.

[bold]SSH errors[/]
This app uses your existing SSH config, keys, and ssh-agent.
For a new host key or login problem, use ssh YOUR_HOST
in a regular PowerShell tab first. Encrypted keys need ssh-agent.
Error details appear below the selected row.

[dim]Esc to return[/]""")

    def action_close(self):
        self.dismiss()


class PortApp(App):
    TITLE = "Port Forward TUI"
    SUB_TITLE = "Saved SSH forwards"
    ENABLE_COMMAND_PALETTE = False
    CSS_PATH = "app.tcss"
    BINDINGS = [
        Binding("n", "new_forward", "New"), Binding("e", "edit_forward", "Edit"),
        Binding("space", "toggle", "On / off"), Binding("d", "delete_forward", "Delete"),
        Binding("r", "restart", "Restart"), Binding("b", "browser", "Browser"),
        Binding("s", "stop_all", "Stop all"), Binding("question_mark", "help", "Help"),
        Binding("q", "request_quit", "Quit"),
        Binding("ctrl+q", "request_quit", "Quit", show=False, priority=True),
        Binding("ctrl+c", "request_quit", "Quit", show=False, priority=True),
        Binding("escape", "list_focus", "Saved list", show=False),
    ]

    def __init__(self, store: Store, manager: TunnelManager):
        super().__init__()
        self.store, self.manager = store, manager
        self.sub_title = store.host
        self.theme = "textual-dark"
        self.last_statuses = None
        self.last_details: dict[str, str] = {}

    def compose(self) -> ComposeResult:
        yield Header()
        with Vertical(id="main"):
            yield Static(f"{self.store.host}   /   this PC -> SSH host", id="destination", markup=False)
            yield Static("QUICK FORWARD", classes="section-label")
            yield Input(placeholder="8000  or  18000:8000  [optional name]", id="quick", max_length=100, select_on_focus=False)
            yield Static("Type a port + Enter to save and start. LOCAL:REMOTE changes the local port.", classes="muted", id="hint")
            yield DataTable(id="forwards", cursor_type="row", zebra_stripes=True)
            yield Static("", id="summary", markup=False)
            yield Static("", id="details", markup=False)
            yield Static("Ready. Select a favorite and press Enter, or just type a port.", id="message", markup=False)
            lifetime = ("BACKGROUND ON | Safe to close Terminal. S stops tunnels; Q closes this UI."
                        if getattr(self.manager, "persistent", False)
                        else "FOREGROUND | Closing this window stops tunnels. Favorites stay saved.")
            yield Static(lifetime, classes="muted", id="lifetime")
        yield Footer()

    def on_mount(self):
        table = self.query_one(DataTable)
        for key, label, width in (("state", "STATE", 12), ("name", "SAVED FORWARD", 26),
                                  ("local", "LOCAL", 8), ("arrow", "->", 3), ("remote", "REMOTE", 8)):
            table.add_column(label, key=key, width=width)
        self.populate()
        table.focus()
        self.set_interval(0.25, self.tick)

    def selected(self) -> Forward | None:
        table = self.query_one(DataTable)
        if 0 <= table.cursor_row < len(self.store.forwards):
            return self.store.forwards[table.cursor_row]
        return None

    def state_text(self, rule: Forward) -> Text:
        state = self.manager.status(rule.id)
        color = {"OFF": "#8391a8", "ON": "#5fe0b1", "CONNECTING": "#efc96a", "ERROR": "#ff838b"}[state]
        return Text(state, style=f"bold {color}")

    def populate(self, select_id: str | None = None):
        table = self.query_one(DataTable)
        previous = self.selected()
        if select_id is None and previous:
            select_id = previous.id
        old_row = table.cursor_row
        table.clear()
        for rule in self.store.forwards:
            table.add_row(self.state_text(rule), Text(rule.name), str(rule.local_port), "->",
                          str(rule.remote_port), key=rule.id)
        target = next((i for i, r in enumerate(self.store.forwards) if r.id == select_id),
                      max(0, min(old_row, len(self.store.forwards) - 1)))
        if self.store.forwards:
            table.move_cursor(row=target)
        self.last_statuses = None
        self.refresh_details()

    def tick(self):
        try:
            self.manager.poll()
        except OSError as error:
            self.say(f"Could not check tunnel status: {error}")
        if self.screen is not self.screen_stack[0]:
            return
        statuses = tuple((r.id, self.manager.status(r.id)) for r in self.store.forwards)
        if statuses != self.last_statuses:
            table = self.query_one(DataTable)
            for rule in self.store.forwards:
                table.update_cell(rule.id, "state", self.state_text(rule))
            self.last_statuses = statuses
        self.refresh_details()

    def refresh_details(self):
        active = sum(self.manager.status(r.id) == "ON" for r in self.store.forwards)
        connecting = sum(self.manager.status(r.id) == "CONNECTING" for r in self.store.forwards)
        self.update_detail("summary", f"{active} active  /  {connecting} connecting  /  {len(self.store.forwards)} saved")
        rule = self.selected()
        if not rule:
            message = "No saved forwards. Type a port to add your first one."
        elif self.manager.status(rule.id) == "ERROR":
            message = self.manager.details(rule.id) or "SSH failed. Press Enter to retry."
        else:
            message = f"{rule.name}:  127.0.0.1:{rule.local_port} -> {self.store.host}:127.0.0.1:{rule.remote_port}"
            details = self.manager.details(rule.id)
            if details:
                message += "\n" + details
            elif self.manager.status(rule.id) == "ON":
                message += "\nTunnel listening. The service must be running on the remote port."
        self.update_detail("details", message)

    def update_detail(self, widget_id: str, message: str):
        if self.last_details.get(widget_id) != message:
            self.query_one(f"#{widget_id}", Static).update(message)
            self.last_details[widget_id] = message

    def say(self, message: str):
        self.query_one("#message", Static).update(message)

    def save_rules(self, rules: list[Forward]) -> bool:
        try:
            self.store.save(rules)
            return True
        except (OSError, ValueError) as error:
            self.say(f"Could not save favorites: {error}")
            return False

    @on(DataTable.RowHighlighted)
    def row_highlighted(self):
        self.refresh_details()

    @on(DataTable.RowSelected)
    def row_selected(self):
        self.action_toggle()

    def on_key(self, event: events.Key):
        if self.screen is self.screen_stack[0] and isinstance(self.focused, DataTable) and event.character and event.character in "0123456789":
            box = self.query_one("#quick", Input)
            box.value = event.character
            box.focus()
            box.cursor_position = len(box.value)
            event.stop()
            event.prevent_default()

    @on(Input.Submitted, "#quick")
    def quick_submit(self, event: Input.Submitted):
        try:
            parts = event.value.strip().split(maxsplit=1)
            if not parts:
                self.action_list_focus()
                return
            local, remote = quick_ports(parts[0])
            name = parts[1] if len(parts) > 1 else ""
            if len(name) > 80:
                raise ValueError("Use a name no longer than 80 characters.")
        except ValueError as error:
            self.say(str(error))
            return
        existing = next((r for r in self.store.forwards if (r.local_port, r.remote_port) == (local, remote)), None)
        rule = existing or Forward.make(local, remote, name)
        if not existing and not self.save_rules(self.store.forwards + [rule]):
            return
        if existing and name:
            rule = replace(existing, name=name)
            if not self.save_rules([rule if r.id == rule.id else r for r in self.store.forwards]):
                return
        self.populate(rule.id)
        self.manager.start(rule)
        event.input.value = ""
        self.action_list_focus()
        self.say(f"Saved: local {local} -> remote {remote}. Enter toggles this forward.")
        self.tick()

    def action_new_forward(self):
        self.query_one("#quick", Input).focus()

    def action_list_focus(self):
        if self.screen is self.screen_stack[0]:
            self.query_one(DataTable).focus()

    def action_toggle(self):
        rule = self.selected()
        if not rule:
            self.action_new_forward()
            return
        if rule.id in self.manager.running:
            self.manager.stop(rule.id)
            self.say(f"Stopped {rule.name}.")
        else:
            self.manager.start(rule)
            self.say(f"Opening local {rule.local_port} -> remote {rule.remote_port}.")
        self.tick()

    def action_restart(self):
        rule = self.selected()
        if rule:
            self.manager.stop(rule.id)
            self.manager.start(rule)
            self.say(f"Reconnecting {rule.name}.")
            self.tick()

    def action_edit_forward(self):
        rule = self.selected()
        if not rule:
            return

        def edited(updated: Forward | None):
            if updated is None:
                return
            if any(r.id != updated.id and (r.local_port, r.remote_port) == (updated.local_port, updated.remote_port)
                   for r in self.store.forwards):
                self.say("That port mapping is already saved. Select the existing favorite instead.")
                return
            active = rule.id in self.manager.running
            if not self.save_rules([updated if r.id == rule.id else r for r in self.store.forwards]):
                return
            if active:
                self.manager.stop(rule.id)
                self.manager.start(updated)
            self.populate(updated.id)
            self.say(f"Saved {updated.name}: local {updated.local_port} -> remote {updated.remote_port}.")

        self.push_screen(EditForward(rule), edited)

    def action_delete_forward(self):
        rule = self.selected()
        if not rule:
            return

        def deleted(confirmed: bool):
            if confirmed and self.save_rules([r for r in self.store.forwards if r.id != rule.id]):
                self.manager.stop(rule.id)
                self.populate()
                self.say(f"Deleted {rule.name}.")

        self.push_screen(Confirm(f"Delete {rule.name} ({rule.local_port} -> {rule.remote_port})? Its tunnel will stop."), deleted)

    def action_browser(self):
        rule = self.selected()
        if rule:
            if self.manager.status(rule.id) != "ON":
                self.say("Start this forward with Enter first, then press B to open its local URL.")
                return
            url = f"http://127.0.0.1:{rule.local_port}"
            webbrowser.open(url)
            self.say(f"Opened {url}")

    def action_stop_all(self):
        self.manager.close()
        self.say("All tunnels stopped. Your favorites are saved.")
        self.tick()

    def action_help(self):
        self.push_screen(Help())

    def action_request_quit(self):
        if getattr(self.manager, "persistent", False):
            self.exit()
        elif self.manager.running:
            self.push_screen(Confirm("Quit and stop all active tunnels? Your favorites stay saved."), self.finish_quit)
        else:
            self.exit()

    def finish_quit(self, confirmed: bool):
        if confirmed:
            self.manager.close()
            self.exit()


def main():
    parser = argparse.ArgumentParser(description="Keyboard port-forward manager with persistent background tunnels")
    parser.add_argument("--data-dir", type=Path, default=DATA_DIR)
    parser.add_argument("--host", help="SSH config alias or user@hostname; saved for future launches")
    parser.add_argument("--foreground", action="store_true", help="Stop tunnels when the UI closes (background is the default)")
    parser.add_argument("--stop-all", action="store_true", help="Stop background tunnels without opening the UI")
    parser.add_argument("--stop-daemon", action="store_true", help="Stop background tunnels and their supervisor")
    parser.add_argument("--check", action="store_true", help="Validate saved settings without opening tunnels")
    options = parser.parse_args()
    lock = manager = daemon_lock = None
    try:
        if options.stop_all or options.stop_daemon:
            from background import exchange
            exchange(options.data_dir, "shutdown" if options.stop_daemon else "stop_all")
            print("Background tunnels stopped.")
            return 0
        if not options.check:
            lock = InstanceLock(options.data_dir)
        store = Store(options.data_dir)
        store.load()
        if options.host:
            validate_host(options.host)
            if store.host != options.host:
                from background import exchange
                try:
                    existing = exchange(options.data_dir, "status")
                except (OSError, ValueError, KeyError):
                    existing = None
                if existing:
                    raise ValueError("Stop the background manager with --stop-daemon before changing hosts.")
                store.host = options.host
                store.save(store.forwards)
        if options.check:
            print(f"OK: {store.host}; {len(store.forwards)} saved forwards; {store.path}")
            return 0
        if not store.host:
            raise ValueError("Choose an SSH target on first launch: app.py --host YOUR_SSH_ALIAS")
        if store.keep_alive and not options.foreground:
            from background import DaemonClient
            manager = DaemonClient(store.host, options.data_dir)
        else:
            daemon_lock = InstanceLock(options.data_dir, "daemon.lock")
            manager = TunnelManager(store.host, options.data_dir)
        PortApp(store, manager).run()
        return 0
    except (OSError, ValueError, KeyError, TypeError, RuntimeError) as error:
        print(f"Port manager: {error}", file=sys.stderr)
        print(f"Settings: {options.data_dir / 'forwards.json'}", file=sys.stderr)
        return 1
    finally:
        if manager and not getattr(manager, "persistent", False):
            manager.close()
        if daemon_lock:
            daemon_lock.close()
        if lock:
            lock.close()


if __name__ == "__main__":
    raise SystemExit(main())
