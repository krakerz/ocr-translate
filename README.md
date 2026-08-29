# ocr-translate

A small Rust tool for Linux: use the tray menu (or bind `ocr-translate
capture` to a key yourself), drag-select a region of the screen, and get the
OCR'd text translated by an LLM or translation API in a popup. Targets
Wayland (GNOME/KDE/wlroots) via the standard xdg-desktop-portal APIs, without
compositor-specific hacks; the codebase is otherwise plain Rust so it also
builds on Windows/macOS with reduced portal/tray support. X11 sessions aren't
supported as a first-class target — see [Known limitations](#known-limitations)
for the one place X11/XWayland is still used, as an implementation detail.

## How it works

1. **Capture**: by default, grabs a screenshot of just the monitor the cursor
   is currently on (so a multi-monitor setup never hands OCR a giant combined
   image) via the `org.freedesktop.portal.Screenshot` xdg-desktop-portal.
   Optionally, an external tool (e.g. KDE's `spectacle`) can do the region
   selection instead — see
   [Alternate capture backend](#alternate-capture-backend-external-tool).
2. **Select** (built-in backend only): shows that screenshot in a borderless
   window; scroll to zoom, right-drag to pan, and left-drag a rectangle to
   crop. This sidesteps the fact that Wayland has no portable "overlay the
   live desktop" API, and zoom makes it practical to select small text precisely.
3. **OCR**: runs the crop through Tesseract (via `leptess`).
4. **Translate**: sends the recognized text to a configured backend — an
   OpenAI-compatible chat API (LM Studio, Ollama, OpenAI, DeepSeek, ...),
   Google Cloud Translation, or Microsoft/Azure ("Bing") Translator — with an
   optional ordered fallback chain if the first one fails. See
   [Providers, fallback, and public/private modes](#providers-fallback-and-publicprivate-modes).
5. **Popup**: shows original + translated text, with a copy button. Each
   successful translation is also recorded to a local history — see
   [History](#history).

The app runs in the system tray with a **Capture** / **Live Clipboard
Translate** / **History** / **Quit** menu (see
[Live Clipboard Translate](#live-clipboard-translate) for the clipboard-based
mode). There's no built-in global hotkey — see [Binding a key](#binding-a-key)
for why, and how to bind one yourself in your DE/compositor instead.

## Requirements

- Rust toolchain
- Tesseract + Leptonica dev libraries (`tesseract`, `leptonica` on most distros)
  and at least one language's tessdata installed
- X11 dev headers — not for capture (that's portal-only now), but `eframe`'s
  window backend keeps an X11 fallback compiled in alongside its Wayland
  backend; also, cursor position and monitor geometry are queried over
  XWayland (see [Known limitations](#known-limitations))
- GTK3 + libappindicator (or libayatana-appindicator) dev libraries, for the
  tray icon:
  ```sh
  # Arch / Manjaro
  sudo pacman -S gtk3 libappindicator-gtk3   # or libayatana-appindicator
  # Debian / Ubuntu
  sudo apt install libgtk-3-dev libappindicator3-dev  # or libayatana-appindicator3-dev
  ```

```sh
cargo build --release
```

## Configuration

Config lives in a per-user directory named `ocr-translation`, resolved the
standard way for your OS: `~/.config/ocr-translation` on Linux,
`%APPDATA%\ocr-translation` on Windows, `~/Library/Application
Support/ocr-translation` on macOS.

**The first time you run the app**, if no config exists yet, it automatically
creates that directory with a ready-to-edit `config.yaml` plus reference
copies `config.example.yaml` and `config.example.conf` (so you can always see
the other format or restore the defaults). You don't need to run anything
first — just start the app and then edit `config.yaml`.

To reset back to the bundled defaults later:

```sh
ocr-translate init-config --format yaml --force   # or: --format conf
```

See [`config/config.example.yaml`](config/config.example.yaml) and
[`config/config.example.conf`](config/config.example.conf) for full,
commented examples of both formats. `.conf` uses INI syntax; each provider is
its own `[provider.<name>]` section.

Key fields:

- `active_provider` / `fallback_providers` — which entries in `providers` to
  try, in order (see [Providers, fallback, and public/private modes](#providers-fallback-and-publicprivate-modes)).
- `providers.<name>.kind` — `openai_compatible` | `google_translate` | `bing_translate`.
- `providers.<name>.mode` — `public` | `private` (`google_translate`/`bing_translate` only).
- `prompt.system` / `prompt.template` — fully customizable prompt sent to
  LLM-based providers. Template placeholders: `{source_lang}`, `{target_lang}`, `{text}`.
- `ocr.languages` — Tesseract language code(s), e.g. `eng`, `eng+jpn`.
- `capture.backend` — `built_in` | `external` (see [Alternate capture backend](#alternate-capture-backend-external-tool)).
- `history.enabled` / `history.max_entries` — see [History](#history).

API keys: prefer `api_key_env: SOME_ENV_VAR` over a literal `api_key` in the
file, so secrets don't end up on disk in plaintext.

## Providers, fallback, and public/private modes

`active_provider` is tried first; if it fails (server not running, bad key,
network error, ...) each entry in `fallback_providers` is tried next, in
order, until one succeeds. This is meant for resilience, e.g. preferring a
local LM Studio server but falling back to a public translation API if it's
not running:

```yaml
active_provider: lmstudio
fallback_providers:
  - google
```

Google Translate and Bing/Azure Translator each support two `mode`s:

- `mode: public` — a free, unofficial, no-key endpoint the provider's own
  translator web page uses. **Only implemented for Google** — it's a genuine,
  widely-used technique (the same one countless open-source translation tools
  rely on) and worked in local testing, though it's undocumented and can be
  rate-limited by Google without notice (you may see this if many requests
  come from the same network). **Bing has no equivalent**: the old free-token
  endpoint (`edge.microsoft.com/translate/auth`) is dead, and the current
  bing.com/translator frontend is guarded by an abuse-prevention check that
  requires replaying a full browser session's cookies to pass — that's
  session spoofing to dodge bot detection, not a stable public API, so it
  isn't implemented; `mode: public` for Bing just returns an explanatory error.
- `mode: private` — the official, authenticated API (Cloud Translation v2 for
  Google, Azure Translator v3 for Bing). Needs `api_key`/`api_key_env` (and
  `region` for Bing, if using a multi-service Azure resource).

Public mode never sends `prompt.system`/`prompt.template` — it's a real
translation API, not an LLM, so there's no prompt to customize.

DeepSeek (an LLM API, not a dedicated translation service) has no free/public
option either — it always requires `api_key_env`.

## Alternate capture backend (external tool)

Instead of the built-in grab-and-crop flow, `capture.backend: external` runs
a configurable shell command and reads the image it leaves on the clipboard,
skipping our own crop window entirely. This is for external tools that do
their own **live** region selection on the real desktop — a nicer experience
than a still-image crop window where it's available. For example, on KDE:

```yaml
capture:
  backend: external
  external_command: "spectacle -r -b -c"
  external_timeout_secs: 10
```

Other examples: `grim -g "$(slurp)" - | wl-copy` (wlroots compositors),
`gnome-screenshot -a -c` (GNOME). This isn't the default — we don't know which
desktop environment you're running, and it depends on a specific tool being
installed — but it's a good option if you're on KDE/Sway/Hyprland/GNOME and
want the nicer selection experience.

## History

Each successful translation is recorded to `history.jsonl` (JSON Lines) next
to your config file, newest last on disk. Controlled by the `history.*`
config keys (`enabled`, `max_entries`, `tray_menu_entries`).

The tray's **History** submenu lists the most recent entries (refreshed every
few seconds, since captures run in a separate process — see below); click one
to reopen it in the popup, or use **Clear History** to delete it all. The
same is available from the CLI:

```sh
ocr-translate show-history 0    # 0 = most recent
ocr-translate clear-history
```

Reopened history entries use the separate `history_popup` config section
(width/height/font_size/always_on_top/auto_close_secs) rather than `popup` —
so you can size the history viewer differently than the live capture-result
popup, e.g. larger for comfortably re-reading older entries.

## Live Clipboard Translate

Doesn't touch the screen at all — instead it watches the *clipboard*: copy
some text anywhere (a browser, a terminal, another app), and a popup shows
its translation within `live_translate.poll_interval_ms` (default 0.5s).
Copy something else and the same popup updates in place; copying the exact
same text again doesn't retranslate. Deliberately **never recorded to
history** — it's meant for quick, disposable lookups, not a log of what you
translated.

Start it from the tray's **Live Clipboard Translate** menu item, or:

```sh
ocr-translate watch-clipboard
```

The popup has a **Show source** checkbox to toggle the original text on/off
(only the translation stays visible when unchecked) — its initial state
comes from `live_translate.show_source_by_default`. Sizing/behavior is
config-only, its own separate section (`live_translate`, same shape as
`popup` plus `show_source_by_default` and `poll_interval_ms`).

## Config hot-reload

While `ocr-translate run` is active, editing the config file takes effect
without restarting it — a background watcher polls it every ~2 seconds and
reloads on change:

- Anything used by a fresh capture or a new Live Clipboard Translate window
  (providers, prompt, OCR, popup sizes, capture backend, history settings,
  `live_translate.*`, ...) applies the next time you trigger one — each runs
  in its own process that reads the config file fresh, so this was already
  true before hot-reload existed. An *already-open* Live Clipboard Translate
  window keeps using the settings it started with until you close and reopen it.
- The tray's History submenu settings (`tray_menu_entries`) apply on its next
  refresh (a couple of seconds), no restart needed.
- A config file that fails to parse while being edited (e.g. mid-save) is
  logged and ignored — the previous valid config keeps running rather than
  crashing the daemon.

## Running

```sh
# Tray daemon (default): sits in the system tray with a
# Capture / Live Clipboard Translate / History / Quit menu.
ocr-translate run

# One-shot: capture, crop, OCR, translate, show popup, exit. Useful for
# binding to a key yourself (see below) or for scripting.
ocr-translate capture

# Watch the clipboard and show a live-updating translation popup (see
# Live Clipboard Translate above). Never recorded to history.
ocr-translate watch-clipboard

# Sanity-check a provider without touching the screen:
ocr-translate test-provider --provider openai "Bonjour le monde"
```

### Binding a key

There's no in-app global hotkey — the only mechanism a Wayland app can use
for one, the `org.freedesktop.portal.GlobalShortcuts` portal, in practice
only grants shortcuts to Flatpak/Snap-sandboxed apps: a plain binary run from
a terminal gets `NotAllowed: An app id is required`, and (confirmed by
testing) wrapping the process in a matching systemd `app-<id>-<random>.scope`
doesn't help either — that's not something fixable from inside the app short
of Flatpak-packaging it. Rather than ship a feature that doesn't work, the
tray's **Capture** menu item and `ocr-translate capture` are *the* way to
trigger a capture; bind the latter to a key yourself, natively in your
DE/compositor (which goes through the compositor's own shortcut system
directly, not the portal, so it always works):

- **KDE Plasma**: System Settings → Shortcuts → add a custom command shortcut
  that runs `ocr-translate capture`.
- **GNOME**: Settings → Keyboard → Keyboard Shortcuts → Custom Shortcuts.
- **Sway / Hyprland / i3**: bind it in your compositor config, e.g. for Sway:
  ```
  bindsym $mod+Shift+o exec ocr-translate capture
  ```

### Running as a systemd user service

```ini
# ~/.config/systemd/user/ocr-translate.service
[Unit]
Description=ocr-translate tray daemon

[Service]
ExecStart=%h/.cargo/bin/ocr-translate run
Restart=on-failure

[Install]
WantedBy=default.target
```

```sh
systemctl --user enable --now ocr-translate.service
```

## Releasing

Pushing a tag like `v0.2.0` triggers [`.github/workflows/release.yml`](.github/workflows/release.yml),
which builds the Linux release binary, packages it into a tarball (with
`README.md`, `LICENSE`, and `config/`), and opens a **draft** GitHub Release
with auto-generated notes and the tarball attached. Review the draft on the
repo's Releases page and publish it manually when it's ready.

```sh
git tag v0.2.0
git push origin v0.2.0
```

## Known limitations

- The crop-selector and popup windows are plain (non-fullscreen, non-overlay)
  windows sized to fit the content — there's no click-through live-desktop
  overlay, since no such API is portable across compositors.
- Active-monitor detection (which monitor to crop the portal's full-desktop
  screenshot down to) queries the cursor position and monitor layout over
  XWayland, via a direct XRandR request — not "X11 support", just the only
  portable way to get either piece of information at all, since Wayland's
  security model deliberately doesn't expose global cursor position or
  output layout to arbitrary clients. XWayland is present on effectively
  every real desktop Wayland session. On a session with no XWayland at all,
  this falls back to using the full multi-monitor screenshot uncropped,
  rather than failing.
- The `external` capture backend detects a fresh capture by diffing the
  clipboard image against what was there before the command ran, so capturing
  the exact same pixels twice in a row reads as "cancelled" the second time —
  not a concern in practice for OCR use.
- The tray's History submenu only refreshes every ~2 seconds (captures run in
  a separate process, so the tray has to notice new entries by re-reading
  `history.jsonl` from disk), so a just-completed capture may take a moment
  to appear there.
