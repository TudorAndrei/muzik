"""Print import status events in the command-line interface."""

from muzik.core.import_models import ImportEvent, LogEvent
from muzik.ui.console import console


class ConsoleImportEvents:
    def emit(self, event: ImportEvent) -> None:
        if isinstance(event, LogEvent):
            console.print(event.message)
