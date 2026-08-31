# Changelog

All notable changes to this project are documented here, following the
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) format. Tracking
starts at 1.10.0 — earlier versions weren't recorded here.

Each entry is a `## [version] - date` header (used by
`.github/workflows/autobuild.yml` to pull the matching section into that
version's GitHub Release notes — keep that format so it stays parseable).

## [1.10.2] - 2026-08-31

### Fixed

- Built-in (portal-based) Linux capture could grab the wrong monitor —
  reported as: capturing while Microsoft Teams was the focused app grabbed
  Teams' screen instead of the one the mouse cursor was actually on. Root
  cause: a non-interactive xdg-desktop-portal `Screenshot` request isn't
  guaranteed to return the full virtual desktop — KDE's portal backend can
  scope it to just the focused window's screen instead, which broke this
  project's "always crop the full desktop down to the monitor under the
  cursor" assumption. Now explicitly requests the `target: Screen` option
  when the installed portal backend advertises support for it (checked via
  `AvailableTargets`, since a backend that doesn't support the option
  rejects the whole request rather than ignoring it). Falls back to the
  prior behavior on older backends. Verified safe and non-regressing via
  real testing, but not reproducible in the dev environment used to fix it
  (that system's portal already returned the full desktop) — if you still
  see this, `RUST_LOG=debug` now logs the portal image size, the resolved
  monitor rect, and whether `Screen` target support was detected, to help
  pin down what's actually happening on your system.

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
