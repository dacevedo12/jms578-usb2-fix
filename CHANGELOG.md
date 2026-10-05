# Changelog

## 0.1.0

- Guided `fix`: validate the adapter, back up, enable USB 2.0-only mode, verify after replug.
- Behind a USB 3 hub, the tool switches that hub port to USB 2.0 while it works and switches it back at the end.
- `restore`, `status` and `backup` commands; `--simulate` for a hardware-free rehearsal.
- macOS support, tested end to end on Apple Silicon. Linux builds and passes tests; real-hardware reports welcome.
