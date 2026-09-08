"""Start the app without importing Textual on the existing-tab return path."""
from port_forward_tui.launch import main

if __name__ == "__main__":
    raise SystemExit(main())
