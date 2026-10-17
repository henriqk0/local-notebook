from __future__ import annotations

from typing import ClassVar

from textual.app import App, ComposeResult
from textual.widgets import Static


class PyNotebookApp(App[None]):
    CSS_PATH = "ui/styles.tcss"
    BINDINGS: ClassVar[list[tuple[str, str, str]]] = [
        ("q", "quit", "Quit"),
    ]

    def compose(self) -> ComposeResult:
        yield Static("pynotebook - Notebook list (placeholder)", id="placeholder")


def main() -> None:
    app = PyNotebookApp()
    app.run()


if __name__ == "__main__":
    main()
