"""A keyboard list grouped by server, using the existing connection forms."""
from rich.text import Text
from textual.widgets import DataTable

from port_forward_tui.connections import SelectedStore
from port_forward_tui.ui import PortApp


class AllMachinesApp(PortApp):
    def __init__(self, manager, catalog):
        self.catalog = catalog
        self.rows = []
        self.last_shape = None
        super().__init__(SelectedStore(manager), manager)

    def configure_columns(self, table):
        for key, label, width in (('machine', 'SERVER', 16), ('state', 'STATE', 10),
                                  ('name', 'SAVED CONNECTION', 20), ('local', 'THIS PC', 7),
                                  ('arrow', '->', 2), ('remote', 'REMOTE', 7)):
            table.add_column(label, key=key, width=width)

    def selected(self):
        row = self.query_one(DataTable).cursor_row
        if 0 <= row < len(self.rows):
            key, rule = self.rows[row]
            if key != self.manager.active_id:
                self.manager.active_id = key
                self.change_registration()
            return rule
        return None

    def change_registration(self):
        if self.view_registration:
            self.view_registration.select_machine(self.manager.active.machine)

    def favorite_candidates(self):
        prefix = self.manager.active_id + ':'
        return [r for r in self.store.forwards if r.id.startswith(prefix)]

    def populate(self, select_id=None):
        table = self.query_one(DataTable)
        previous = self.selected()
        select_id = select_id or (previous.id if previous else None)
        previous_machine = self.manager.active_id
        old_row = table.cursor_row
        table.clear()
        self.rows = []
        for key in self.manager.order:
            entry = self.manager.entries[key]
            rules = [r for r in self.store.forwards if r.id.startswith(key + ':')]
            for rule in rules:
                table.add_row(Text(entry.machine.name, style='bold cyan'), self.state_text(rule), Text(rule.name),
                              str(rule.local_port), '->', str(rule.remote_port), key=rule.id)
                self.rows.append((key, rule))
            if not rules:
                table.add_row(Text(entry.machine.name, style='bold cyan'), '', Text('No favorites — press A', style='dim'),
                              '', '', '', key=key + ':empty')
                self.rows.append((key, None))
        target = next((i for i, (_, rule) in enumerate(self.rows) if rule and rule.id == select_id), None)
        if target is None:
            target = next((i for i, (key, _) in enumerate(self.rows) if key == previous_machine), min(old_row, max(0, len(self.rows)-1)))
        if self.rows:
            table.move_cursor(row=target)
        self.last_statuses = None
        self.refresh_details()

    def sync_favorites(self, select_id=None):
        forwards = self.manager.forwards
        shape = (tuple(self.manager.order), tuple(forwards))
        if shape != self.last_shape:
            self.last_shape = shape
            self.store.forwards = forwards
            self.populate(select_id)

    def refresh_details(self):
        self.selected()
        super().refresh_details()
        machine = self.manager.active.machine
        self.sub_title = f'{len(self.manager.order)} servers | {machine.name}'
        self.update_detail('destination', f'All servers  |  Add to: {machine.name} ({machine.target})  |  H: manage machines')
        self.update_detail('hint', 'Up/Down selects a server’s connection. A or a port number adds to that server. Same local port already used? Set another with E.')
        if self.manager.catalog_error:
            self.say('Could not refresh saved machines: ' + self.manager.catalog_error)

    def action_stop_all(self):
        try:
            super().action_stop_all()
        except OSError as error:
            self.say(str(error))

    def action_toggle(self):
        try:
            super().action_toggle()
        except OSError as error:
            self.say(str(error))

    def action_restart(self):
        try:
            super().action_restart()
        except OSError as error:
            self.say(str(error))
