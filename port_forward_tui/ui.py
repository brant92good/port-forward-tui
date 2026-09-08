"""Keyboard-first local port forwarding for Windows Terminal."""
from __future__ import annotations

# Existing-view shortcuts must not import Textual just to select a tab.
if __name__ == "__main__":
    from port_forward_tui.launch import main as launch_main
    raise SystemExit(launch_main())

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
from textual.widgets import Button, DataTable, Footer, Header, Input, Label, OptionList, Static

from port_forward_tui.forwarding import DATA_DIR, Forward, InstanceLock, Store, TunnelManager, port, quick_ports, validate_host, requested
from port_forward_tui.focus_settings import SCOPES, SCOPE_LABELS, read_scope, save_scope
from port_forward_tui.error_help import connection_help


class Settings(ModalScreen[str | None]):
    BINDINGS = [Binding("escape", "cancel", "Cancel")]

    def __init__(self, directory: Path):
        super().__init__()
        self.directory = directory
        self.scope = read_scope(directory)

    def compose(self) -> ComposeResult:
        with Vertical(id="settings-dialog"):
            yield Static("SETTINGS", classes="dialog-title")
            yield Label("When I return to the app, look for my last-used tab in:")
            yield OptionList(*SCOPE_LABELS, id="focus-scope")
            yield Static("Up / Down chooses. Enter saves. Esc cancels.\n"
                         "This affects return shortcuts for the selected machine.\n"
                         "If no matching tab is found, a new one opens here.", classes="muted")
            yield Static("", id="settings-error", markup=False)

    def on_mount(self):
        options = self.query_one(OptionList)
        options.highlighted = SCOPES.index(self.scope)
        options.focus()

    @on(OptionList.OptionSelected)
    def selected_scope(self, event: OptionList.OptionSelected):
        scope = SCOPES[event.option_index]
        try:
            save_scope(self.directory, scope)
        except (OSError, ValueError) as error:
            self.query_one("#settings-error", Static).update(str(error))
            return
        self.dismiss(scope)

    def action_cancel(self):
        self.dismiss(None)


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
    # Set the initial target while mounting. Widget.focus() is deferred and can
    # route a fast first key to Name, or reselect the first typed digit later.
    AUTO_FOCUS = "#local"
    BINDINGS = [Binding("escape", "cancel", "Cancel"), Binding("ctrl+s", "save", "Save", priority=True)]

    def __init__(self, rule: Forward, create=False, machine_label=''):
        super().__init__()
        self.rule = rule
        self.create = create
        self.machine_label = machine_label
        if create:
            self.AUTO_FOCUS = "#remote"

    def compose(self) -> ComposeResult:
        with VerticalScroll(id="edit-dialog"):
            yield Label("ADD A CONNECTION" if self.create else "EDIT SAVED CONNECTION", classes="dialog-title")
            if self.machine_label:
                yield Static('Server: ' + self.machine_label, markup=False, classes='muted')
            if not self.create:
                yield Label("Name")
                yield Input(self.rule.name, id="name", max_length=80)
            yield Label("Port used by the app on your REMOTE computer")
            yield Input("" if self.create else str(self.rule.remote_port), placeholder="Example: 8000", id="remote", max_length=5)
            yield Label("Port on THIS computer (leave blank to use the same number)")
            yield Input("" if self.create else str(self.rule.local_port), id="local", max_length=5)
            if self.create:
                yield Label("Name (optional)")
                yield Input("", placeholder="Example: My web app", id="name", max_length=80)
            yield Static("Tab moves between fields. Enter or Ctrl+S " +
                         ("saves and connects." if self.create else "saves; a connected favorite restarts."), classes="muted")
            yield Static("", id="edit-error", markup=False)
            with Horizontal(classes="buttons"):
                yield Button("Save and connect" if self.create else "Save", variant="primary", id="save")
                yield Button("Cancel", id="cancel")

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

[bold]Open a remote app on this computer[/]
A port is the number in an app address, such as 8000 in localhost:8000.
This app connects a port here to an app on your remote computer using SSH.
The remote app must already be running; this tool does not start it.

Press A for a form, or type a number then Enter to save and connect.
  8000          this computer 8000 -> remote computer 8000
  18000:8000    this computer 18000 -> remote computer 8000
  8888 Jupyter  optional friendly name after a space

[bold]Keyboard[/]
Up / Down     Select a saved forward
Enter / Space Start or stop the selected forward
N             Focus the quick-forward box
A             Add a connection using a form
E             Edit name and ports (local port is selected)
D             Delete a saved forward
R             Restart the selected forward
B             Open http://127.0.0.1:LOCAL in your browser
S             Stop all tunnels
Esc           Return from quick entry to the saved list
Q / Ctrl+Q    Close the UI (background tunnels keep running)
F2            Settings: return to a view here or across all Terminal windows
H             Add/import machines or choose which server to select
?             This help

Saved favorites start OFF until you choose them.
Started connections retry network failures automatically (2s up to 30s delay).
RETRYING means waiting for another attempt. Enter stops it; R retries now.
Login, host-key and occupied local-port errors need your attention.
Background mode is ON by default: you may close the entire Terminal app.
Reopen the UI to manage the same running tunnels. S explicitly stops all.
Multiple views can attach at once; favorites and tunnel state stay in sync.
With --foreground, closing the UI stops its tunnels instead.
Signing out or rebooting ends the tunnels; favorites remain saved.
After Wi-Fi or VPN returns, interrupted connections retry in the background.
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
        Binding("a", "add_form", "Add form"),
        Binding("n", "new_forward", "Quick entry", show=False), Binding("e", "edit_forward", "Edit"),
        Binding("space", "toggle", "On / off"), Binding("d", "delete_forward", "Delete"),
        Binding("r", "restart", "Restart", show=False), Binding("b", "browser", "Browser"),
        Binding("s", "stop_all", "Stop all"), Binding("question_mark", "help", "Help"),
        Binding("f2", "settings", "Settings", show=False),
        Binding("h", "machines", "Machines"),
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
        self.view_registration = None

    def compose(self) -> ComposeResult:
        yield Header()
        with Vertical(id="main"):
            yield Static(f"Machine: {self.store.machine_name or self.store.host}  |  {self.store.host}  |  H: change machine", id="destination", markup=False)
            yield Static("CONNECT TO A REMOTE APP", classes="section-label")
            yield Input(placeholder="8000  or  18000:8000  [optional name]", id="quick", max_length=100, select_on_focus=False)
            yield Static("Type the remote app's port + Enter, or press A for a form. ? explains ports.", classes="muted", id="hint")
            yield DataTable(id="forwards", cursor_type="row", zebra_stripes=True)
            yield Static("", id="summary", markup=False)
            yield Static("", id="details", markup=False)
            yield Static("Ready. Select a favorite and press Enter, or just type a port.", id="message", markup=False)
            lifetime = ("Keeps running when Terminal closes. S stops connections; Q closes this screen."
                        if getattr(self.manager, "persistent", False)
                        else "FOREGROUND | Closing this window stops tunnels. Favorites stay saved.")
            yield Static(lifetime, classes="muted", id="lifetime")
        yield Footer()

    def on_mount(self):
        table = self.query_one(DataTable)
        self.configure_columns(table)
        self.populate()
        self.sync_favorites()
        table.focus()
        self.set_interval(0.25, self.tick)

    def configure_columns(self, table):
        for key, label, width in (("state", "STATE", 12), ("name", "SAVED CONNECTION", 26),
                                  ("local", "THIS PC", 8), ("arrow", "->", 3), ("remote", "REMOTE PC", 10)):
            table.add_column(label, key=key, width=width)

    def selected(self) -> Forward | None:
        table = self.query_one(DataTable)
        if 0 <= table.cursor_row < len(self.store.forwards):
            return self.store.forwards[table.cursor_row]
        return None

    def state_text(self, rule: Forward) -> Text:
        state = self.manager.status(rule.id)
        color = {"OFF": "#8391a8", "ON": "#5fe0b1", "CONNECTING": "#efc96a", "RETRYING": "#efc96a", "ERROR": "#ff838b"}.get(state, '#8391a8')
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
        # A queued timer can fire while Textual is removing the closing view.
        if not self.is_running:
            return
        try:
            self.manager.poll()
        except OSError as error:
            self.say(f"Could not check tunnel status: {error}")
        if self.screen is not self.screen_stack[0]:
            return
        self.sync_favorites()
        statuses = tuple((r.id, self.manager.status(r.id)) for r in self.store.forwards)
        if statuses != self.last_statuses:
            table = self.query_one(DataTable)
            for rule in self.store.forwards:
                table.update_cell(rule.id, "state", self.state_text(rule))
            self.last_statuses = statuses
        self.refresh_details()

    def sync_favorites(self, select_id: str | None = None):
        if not getattr(self.manager, "shared_favorites", False):
            return
        if self.store.forwards != self.manager.forwards:
            previous = self.selected()
            self.store.forwards = list(self.manager.forwards)
            self.populate(select_id or (previous.id if previous else None))

    def on_app_focus(self):
        if self.view_registration:
            self.view_registration.focused()

    def refresh_details(self):
        active = sum(self.manager.status(r.id) == "ON" for r in self.store.forwards)
        connecting = sum(self.manager.status(r.id) == "CONNECTING" for r in self.store.forwards)
        retrying = sum(self.manager.status(r.id) == "RETRYING" for r in self.store.forwards)
        self.update_detail("summary", f"{active} active  /  {connecting} connecting  /  {retrying} retrying  /  {len(self.store.forwards)} saved")
        rule = self.selected()
        if not rule:
            message = "No saved connections. Press A for a form, or type your remote app's port."
        elif self.manager.status(rule.id) == "ERROR":
            details = self.manager.details(rule.id)
            message = connection_help(details) + "\n" + details
        else:
            message = f"This computer: http://localhost:{rule.local_port}  ->  Remote app: port {rule.remote_port}"
            details = self.manager.details(rule.id)
            if details:
                message += "\n" + details
            elif self.manager.status(rule.id) == "ON":
                message += "\nPress B to open a web app. ON means the connection is listening; the remote app must also be running."
            else:
                message += "\nPress Enter to connect. The remote app must already be running."
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
        existing = next((r for r in self.favorite_candidates() if (r.local_port, r.remote_port) == (local, remote)), None)
        rule = existing or Forward.make(local, remote, name)
        if getattr(self.manager, "shared_favorites", False):
            if existing and name:
                rule = replace(existing, name=name)
            try:
                rule = self.manager.upsert(rule, expected=existing, start=True)
            except OSError as error:
                self.say(str(error))
                return
            self.sync_favorites(rule.id)
        else:
            if not existing and not self.save_rules(self.store.forwards + [rule]):
                return
            if existing and name:
                rule = replace(existing, name=name)
                if not self.save_rules([rule if r.id == rule.id else r for r in self.store.forwards]):
                    return
            self.manager.start(rule)
        self.populate(rule.id)
        event.input.value = ""
        self.action_list_focus()
        self.say(f"Saved: local {local} -> remote {remote}. Enter toggles this forward.")
        self.tick()

    def action_new_forward(self):
        self.query_one("#quick", Input).focus()

    def favorite_candidates(self):
        return self.store.forwards

    def action_add_form(self):
        def added(rule: Forward | None):
            if rule is None:
                return
            existing = next((r for r in self.favorite_candidates() if
                             (r.local_port, r.remote_port) == (rule.local_port, rule.remote_port)), None)
            if existing:
                rule = existing
            if getattr(self.manager, "shared_favorites", False):
                try:
                    rule = self.manager.upsert(rule, expected=existing, start=True)
                except OSError as error:
                    self.say(str(error))
                    return
                self.sync_favorites(rule.id)
            else:
                if not existing and not self.save_rules(self.store.forwards + [rule]):
                    return
                self.manager.start(rule)
            self.populate(rule.id)
            self.say(f"Saved {rule.name}. Enter starts or stops this connection.")
            self.tick()
        self.push_screen(EditForward(Forward.make(8000, 8000), create=True,
                                     machine_label=self.store.machine_name or self.store.host), added)

    def action_list_focus(self):
        if self.screen is self.screen_stack[0]:
            self.query_one(DataTable).focus()

    def action_toggle(self):
        rule = self.selected()
        if not rule:
            self.action_new_forward()
            return
        if requested(self.manager, rule.id):
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

    async def action_edit_forward(self):
        rule = self.selected()
        if not rule:
            return

        def edited(updated: Forward | None):
            if updated is None:
                return
            if getattr(self.manager, "shared_favorites", False):
                try:
                    self.manager.upsert(updated, expected=rule)
                except OSError as error:
                    self.say(str(error))
                    return
                self.sync_favorites(updated.id)
                self.populate(updated.id)
                self.say(f"Saved {updated.name}: local {updated.local_port} -> remote {updated.remote_port}.")
                return
            if any(r.id != updated.id and (r.local_port, r.remote_port) == (updated.local_port, updated.remote_port)
                   for r in self.store.forwards):
                self.say("That port mapping is already saved. Select the existing favorite instead.")
                return
            active = requested(self.manager, rule.id)
            if not self.save_rules([updated if r.id == rule.id else r for r in self.store.forwards]):
                return
            if active:
                self.manager.stop(rule.id)
                self.manager.start(updated)
            self.populate(updated.id)
            self.say(f"Saved {updated.name}: local {updated.local_port} -> remote {updated.remote_port}.")

        await self.push_screen(EditForward(rule, machine_label=self.store.machine_name or self.store.host), edited)

    def action_delete_forward(self):
        rule = self.selected()
        if not rule:
            return

        def deleted(confirmed: bool):
            if not confirmed:
                return
            if getattr(self.manager, "shared_favorites", False):
                try:
                    self.manager.delete(rule)
                except OSError as error:
                    self.say(str(error))
                    return
                self.sync_favorites()
                self.say(f"Deleted {rule.name}.")
            elif self.save_rules([r for r in self.store.forwards if r.id != rule.id]):
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

    async def action_settings(self):
        def saved(scope: str | None):
            if scope:
                self.say("Focus scope saved: " + SCOPE_LABELS[SCOPES.index(scope)])
        try:
            await self.push_screen(Settings(self.store.directory), saved)
        except (OSError, ValueError) as error:
            self.say(f"Could not open settings: {error}")

    def action_request_quit(self):
        if getattr(self.manager, "persistent", False):
            self.exit()
        elif any(requested(self.manager, r.id) for r in self.store.forwards):
            self.push_screen(Confirm("Quit and stop all active tunnels? Your favorites stay saved."), self.finish_quit)
        else:
            self.exit()

    def action_machines(self):
        def switch(confirmed=True):
            if confirmed:
                self.exit('pick-machine')
        if not getattr(self.manager, 'persistent', False) and any(requested(self.manager, r.id) for r in self.store.forwards):
            self.push_screen(Confirm('Switch machine and stop this foreground view\'s connections? Favorites stay saved.'), switch)
        else:
            switch()

    def finish_quit(self, confirmed: bool):
        if confirmed:
            self.manager.close()
            self.exit()


def main():
    """Compatibility entry point for callers importing the TUI module."""
    from port_forward_tui.launch import main as launch_main
    return launch_main(PortApp)
