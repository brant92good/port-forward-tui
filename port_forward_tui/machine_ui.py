"""Keyboard machine selection and explicit SSH configuration import."""
from pathlib import Path

from rich.text import Text
from textual import on
from textual.app import App, ComposeResult
from textual.binding import Binding
from textual.containers import Vertical, VerticalScroll, Horizontal
from textual.screen import ModalScreen
from textual.widgets import Header, Footer, Static, Label, Input, Button, OptionList, SelectionList

from port_forward_tui.machines import Catalog, Machine, import_ssh, ssh_aliases
from port_forward_tui.forwarding import port


class AddMachine(ModalScreen[Machine | None]):
    AUTO_FOCUS = '#target'
    BINDINGS = [Binding('escape', 'cancel', 'Cancel'), Binding('ctrl+s', 'save', 'Save', priority=True)]

    def __init__(self, catalog):
        super().__init__()
        self.catalog = catalog

    def compose(self):
        with VerticalScroll(id='machine-form'):
            yield Label('ADD A MACHINE', classes='dialog-title')
            yield Label('SSH name or user@address')
            yield Input(placeholder='workbox  or  alex@server.example.com', id='target')
            yield Label('Name (optional)')
            yield Input(placeholder='My development server', id='machine-name', max_length=80)
            yield Label('SSH login port (optional; blank uses your SSH settings, normally 22)')
            yield Input(placeholder='22', id='ssh-port', max_length=5)
            yield Static('This is the login port, not your web app\'s port.\n'
                         'Uses your existing SSH keys / agent. Saving does not connect.', classes='muted')
            yield Static('', id='machine-error', markup=False)
            with Horizontal(classes='buttons'):
                yield Button('Save machine', id='save-machine', variant='primary')
                yield Button('Cancel', id='cancel-machine')

    def action_cancel(self):
        self.dismiss(None)

    def action_save(self):
        try:
            value = self.query_one('#ssh-port', Input).value.strip()
            machine = self.catalog.add(self.query_one('#target', Input).value,
                                       self.query_one('#machine-name', Input).value,
                                       port(value) if value else None)
            self.dismiss(machine)
        except (OSError, ValueError) as error:
            self.query_one('#machine-error', Static).update(str(error))

    @on(Input.Submitted)
    def submitted(self):
        self.action_save()

    @on(Button.Pressed)
    def pressed(self, event):
        self.action_save() if event.button.id == 'save-machine' else self.action_cancel()


class ImportMachines(ModalScreen[list[Machine] | None]):
    BINDINGS = [Binding('escape', 'cancel', 'Cancel'), Binding('ctrl+s', 'save', 'Import selected', priority=True)]

    def __init__(self, catalog):
        super().__init__()
        self.catalog = catalog
        self.scanned = None

    def compose(self):
        with VerticalScroll(id='machine-import'):
            yield Label('IMPORT FROM SSH CONFIG', classes='dialog-title')
            yield Label('Configuration file (your keys stay where they are)')
            yield Input(str(Path.home() / '.ssh/config'), id='config-path')
            yield Button('Read host names', id='scan-config')
            yield SelectionList(id='import-hosts')
            yield Static('Space selects a machine. Ctrl+S imports the selected names.\n'
                         'No login is attempted. OpenSSH still handles keys, jumps and other settings.', classes='muted')
            yield Static('', id='import-error', markup=False)
            with Horizontal(classes='buttons'):
                yield Button('Import selected', id='import-selected', variant='primary')
                yield Button('Cancel', id='cancel-import')

    def on_mount(self):
        self.scan()

    def scan(self):
        items = self.query_one('#import-hosts', SelectionList)
        items.clear_options()
        self.scanned = None
        try:
            path = Path(self.query_one('#config-path', Input).value).expanduser().resolve()
            aliases = ssh_aliases(path)
            items.add_options([(Text(alias), alias, False) for alias in aliases])
            if aliases:
                items.highlighted = 0
            self.scanned = path
            self.query_one('#import-error', Static).update('' if aliases else 'No literal Host names found. Add a machine manually instead.')
            items.focus()
        except (OSError, ValueError) as error:
            self.query_one('#import-error', Static).update(str(error))

    def action_save(self):
        try:
            if self.scanned != Path(self.query_one('#config-path', Input).value).expanduser().resolve():
                raise ValueError('Read host names again after changing the file path.')
            selected = self.query_one('#import-hosts', SelectionList).selected
            if not selected:
                raise ValueError('Select a machine with Space first.')
            self.dismiss(import_ssh(self.catalog, self.scanned, selected))
        except (OSError, ValueError) as error:
            self.query_one('#import-error', Static).update(str(error))

    def action_cancel(self):
        self.dismiss(None)

    @on(Input.Submitted)
    def submitted(self):
        self.scan()

    @on(Button.Pressed)
    def pressed(self, event):
        if event.button.id == 'scan-config':
            self.scan()
        elif event.button.id == 'import-selected':
            self.action_save()
        else:
            self.action_cancel()


class MachinePicker(App[str | None]):
    TITLE = 'Machines'
    ENABLE_COMMAND_PALETTE = False
    CSS_PATH = 'app.tcss'
    BINDINGS = [Binding('a', 'add', 'Add machine'), Binding('i', 'import_hosts', 'Import SSH config'),
                Binding('slash', 'search', 'Search'), Binding('escape', 'list_focus', 'List'),
                Binding('q', 'quit', 'Cancel'), Binding('ctrl+q', 'quit', 'Cancel', priority=True)]

    def __init__(self, catalog: Catalog, purpose='Choose a machine to manage its ports'):
        super().__init__()
        self.catalog, self.purpose = catalog, purpose
        self.items = []

    def compose(self) -> ComposeResult:
        yield Header()
        with Vertical(id='machine-picker'):
            yield Static(self.purpose, classes='dialog-title', markup=False)
            yield Static('Each machine keeps its own favorites. Existing background connections keep running.', classes='muted')
            yield Input(placeholder='Filter machines by name or address', id='machine-search')
            yield OptionList(id='machines')
            yield Static('A adds a machine. I imports names from your SSH config. Enter opens the selected machine.', id='picker-message', markup=False)
        yield Footer()

    def on_mount(self):
        self.refresh_machines()
        self.action_list_focus()

    def refresh_machines(self):
        try:
            query = self.query_one('#machine-search', Input).value.casefold()
            self.items = [m for m in self.catalog.list() if query in (m.name + ' ' + m.target).casefold()]
            widget = self.query_one('#machines', OptionList)
            widget.clear_options()
            widget.add_options([Text(f'{m.name}  |  {m.target}' + (f'  SSH port {m.ssh_port}' if m.ssh_port else '')) for m in self.items])
            if self.items:
                widget.highlighted = 0
            self.query_one('#picker-message', Static).update(
                'Enter opens the selected machine. A adds, I imports.' if self.items
                else 'No machines here yet. Press A to add one, or I to import your SSH config.')
        except (OSError, ValueError) as error:
            self.query_one('#picker-message', Static).update(str(error))

    @on(Input.Changed, '#machine-search')
    def changed(self):
        self.refresh_machines()

    @on(Input.Submitted, '#machine-search')
    def searched(self):
        self.action_list_focus()

    @on(OptionList.OptionSelected, '#machines')
    def selected(self, event):
        self.exit(self.items[event.option_index].id)

    def action_add(self):
        def added(machine):
            if machine:
                self.exit(machine.id)
        self.push_screen(AddMachine(self.catalog), added)

    def action_import_hosts(self):
        self.push_screen(ImportMachines(self.catalog), lambda _: self.refresh_machines())

    def action_search(self):
        self.query_one('#machine-search', Input).focus()

    def action_list_focus(self):
        self.query_one('#machines', OptionList).focus()


def pick_machine(catalog, purpose='Choose a machine to manage its ports'):
    return MachinePicker(catalog, purpose).run()
