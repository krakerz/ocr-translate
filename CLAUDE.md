# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

`ocr-translate`: a Linux-first Rust tool that captures a screen region, OCRs it (Tesseract), sends the text to a configurable translation/LLM backend, and shows the result in a popup. Runs as a system tray app with a Capture/Quit menu, plus best-effort global-hotkey support. Built to work across both X11 and Wayland (GNOME/KDE/wlroots) without compositor-specific hacks; also builds on Windows/macOS with reduced hotkey/portal/tray support.

## Build & run

```sh
cargo build --release
cargo build              # debug, faster iteration
cargo fmt                # always run before considering an edit done
cargo test               # no test suite exists yet; this just typechecks
```

There is no lint config beyond rustfmt — no clippy CI, no test suite. When changing behavior, verify manually (see "Manual verification" below); `cargo build` succeeding is not sufficient evidence a fix works, especially for anything touching capture/tray/hotkey code.

System dependencies (see README.md for distro package names): Tesseract + Leptonica dev libs, X11 dev headers, GTK3 + libappindicator/libayatana-appindicator dev libs (tray icon).

CLI surface (`src/main.rs`): `ocr-translate run` (default — tray + hotkey daemon), `capture` (one-shot: capture → crop → OCR → translate → popup), `test-provider [--provider NAME] TEXT` (exercises a translate backend without touching the screen — the fastest way to test provider/prompt changes), `init-config --format yaml|conf [--force]` (regenerates the default config; normally auto-created on first run).

## Architecture

**Process model is the load-bearing design decision here.** `ocr-translate run` (`src/daemon.rs`) starts a tray icon (GTK main loop, its own thread — `src/tray.rs`) and hotkey listener threads (`src/hotkey.rs`), then blocks on a channel for `DaemonEvent::{Capture, Quit}`. Critically, **`Capture` does not run the capture pipeline in-process** — it spawns `ocr-translate capture` as a fresh child process (`daemon::spawn_capture`). This is not incidental: GTK's main loop (tray) and eframe/winit (the crop-selector/popup windows) both talk to the X11/Wayland connection, and doing both in one process on different threads was found to silently hang with no error. If you're tempted to inline that spawn back into the daemon process, don't — re-read the doc comment on `daemon::run` first.

**The actual one-shot pipeline** lives in `main.rs::run_capture_cycle_inner` and is the same code path whether invoked via `ocr-translate capture`, spawned by the daemon, or (indirectly) by a hotkey: `capture::grab_active_monitor()` → `capture::select_crop()` → `ocr::recognize()` → `translate::build(cfg).translate()` → `popup::show_result()`. Any error surfaces via `popup::show_error()` too, since this typically runs headless (triggered by a hotkey/tray, no attached terminal).

**Capture backend selection** (`src/capture/mod.rs`) branches on `WAYLAND_DISPLAY`/`XDG_SESSION_TYPE`: Wayland → `capture/portal.rs` (xdg-desktop-portal `Screenshot`, the only portable way to get pixels on Wayland), X11 → `capture/x11.rs` (direct `xcap` grab). Both crop down to just the monitor under the cursor rather than handing OCR the whole multi-monitor desktop; `capture/pointer.rs` gets cursor position via a direct X11/XWayland query (works even under Wayland compositors, since XWayland mirrors the real cursor position — there's no portable Wayland API for this). **`xcap` is pinned to `0.0.14`** (see comment in `Cargo.toml`) — later 0.x versions changed `Monitor::x()/y()/width()/height()` to internally call a Wayland scale-factor lookup that hangs indefinitely on at least this project's KDE/KWin test setup. Do not bump that dependency without re-verifying capture actually completes on a real Wayland session, not just that it compiles.

There is no portable "overlay the live desktop" API on either X11 or Wayland, so region selection (`capture/selector.rs`) works by grabbing a full still image first, then showing it in a plain eframe window with its own zoom (scroll)/pan (right-drag)/select (left-drag) logic — coordinate transforms between screen space and image space live entirely in `SelectorApp`.

**Hotkeys are inherently best-effort** (`src/hotkey.rs`): X11 gets a direct `global-hotkey` grab; Wayland tries the `GlobalShortcuts` xdg-desktop-portal, which in practice requires Flatpak/Snap sandboxing and will log `NotAllowed: An app id is required` for a plain binary — this is a portal-side restriction, not a bug, and is not fixable by changing how the process is launched (systemd scope/service naming does not help; confirmed by testing). The tray's Capture menu item and `ocr-translate capture` bound as a native DE/WM shortcut are the reliable fallbacks in every case; don't try to "fix" the portal path further without new evidence it's actually fixable.

**Config** (`src/config.rs`): lives in `~/.config/ocr-translation/` (Linux; `dirs::config_dir()` gives the OS-correct equivalent elsewhere), auto-created with a starter `config.yaml` plus `config.example.{yaml,conf}` reference copies on first run if missing. Both `.yaml` (via `serde_yaml`, full nested structure with `#[serde(default)]` at every level) and `.conf` (hand-written INI parsing in `load_ini`, since INI has no native nested-struct mapping) must be kept in sync manually — if you add a config field, update `AppConfig`/the relevant sub-struct *and* `load_ini`'s match arms *and* both example files in `config/`. `ProviderConfig.kind` (`openai_compatible` | `google_translate` | `bing_translate`) drives which `translate/*.rs` implementation gets built in `translate::build_for`.

**Translation providers** (`src/translate/`) all implement the small `Translator` trait. `openai_compat.rs` is the general-purpose one — it's how LM Studio, Ollama, OpenAI, and DeepSeek are all supported through a single implementation (any `/chat/completions`-shaped API), configured entirely via `ProviderConfig` (base_url/model/api_key) rather than needing provider-specific code. `google.rs`/`bing.rs` are real translation APIs (not LLMs), added as separate implementations because their request/response shapes are unrelated to the chat-completions format.

## Manual verification

There's no automated test suite, so behavior changes need to be checked by hand:
- `cargo build && ./target/debug/ocr-translate capture` exercises the full pipeline directly (fastest loop for capture/OCR/crop-selector changes).
- `ocr-translate test-provider --provider NAME "some text"` exercises just the translate leg without any screen interaction — use this for prompt/provider changes.
- Tray behavior needs an actual running desktop; a `com.canonical.dbusmenu` `Event` D-Bus call against the tray's menu object path (visible via `busctl --user` / the `org.kde.StatusNotifierWatcher`'s `RegisteredStatusNotifierItems`) can simulate a real menu click without needing to drive a mouse, which is how tray wiring has been verified in this environment.
- Kill stray `ocr-translate` processes between test runs (`pkill -f ocr-translate`) — a killed-without-cleanup tray process can leave a stale, non-functional icon visible in the system tray that's easy to mistake for a live one.
