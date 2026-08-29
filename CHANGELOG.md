# Changelog

All notable changes to this project are documented here, following the
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) format. Tracking
starts at 1.10.0 — earlier versions weren't recorded here.

Each entry is a `## [version] - date` header (used by
`.github/workflows/autobuild.yml` to pull the matching section into that
version's GitHub Release notes — keep that format so it stays parseable).

## [1.10.1] - 2026-08-29

### Fixed

- A real hang from Live Region Translate's Quick Capture running at the
  same time as its background region-watching thread — both could call
  into Tesseract concurrently, which its C++ API isn't safe for. All
  Tesseract calls are now serialized process-wide.

## [1.10.0] - 2026-08-29

### Added

- Live Region Translate: watch multiple screen regions at once via
  "+ Add Region" (previously-selected regions are outlined and labeled on
  the picker so you can see where they are while adding another).
- Live Region Translate: "Quick Capture" — a one-shot capture (identical
  pipeline to `capture`, including history) shown inline at the top of the
  list instead of a separate popup, toggleable.
- Live Region Translate: "Show Regions" — a read-only preview of every
  currently-watched region, labeled directly on the image.
- Live Region Translate: delete or rename a region directly from its
  window.
- New CLI subcommands so the above can be bound to hotkeys the same way
  `capture` already is: `region-capture`, `region-show`,
  `region-delete <ID>`, `region-rename <ID> <NAME>`.
- A conflict prompt (Cancel / Stop it and continue) between `capture`,
  Live Clipboard Translate, and Live Region Translate — only
  Live Clipboard Translate + one-shot `capture` can run at the same time.
- Windows support: one-shot capture, the tray, Live Clipboard Translate,
  and Live Region Translate all confirmed working end-to-end.
- Windows file logging (`ocr-translate.log` next to the config file), since
  the app has no console window there.
- Packaged releases bundle Tesseract data for English, Japanese, and
  vertical Japanese (`jpn_vert` — e.g. for manga) — no manual tessdata setup
  needed out of the box.
- `psm` documented in the example config, including the vertical-text case.

### Changed

- `autobuild.yml` now creates a new draft GitHub Release per build (tagged
  with the version and run number) instead of overwriting the same one.
