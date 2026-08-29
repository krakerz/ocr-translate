# ocr-translate

A small tray tool for Linux (Wayland) and Windows: drag-select a region of
the screen, and get the OCR'd text translated by an LLM or translation API in
a popup. macOS isn't supported.

> Just want to run it? Grab a prebuilt binary from the
> [Releases page](../../releases) — see [Installing](#installing). The
> [Requirements](#requirements) section further down is only for building it
> yourself from source.

## How it works

1. **Capture**: grabs a screenshot of the monitor the cursor is on.
2. **Select**: shows that screenshot in a zoom/pan window — scroll to zoom,
   right-drag to pan, left-drag a rectangle to crop.
3. **OCR**: runs the crop through Tesseract.
4. **Translate**: sends the recognized text to a configured backend — an
   OpenAI-compatible chat API (LM Studio, Ollama, OpenAI, DeepSeek, ...),
   Google Cloud Translation, Microsoft/Azure ("Bing") Translator, or DeepL —
   with an optional ordered fallback chain if the first one fails. See
   [Providers, fallback, and public/private modes](#providers-fallback-and-publicprivate-modes).
5. **Popup**: shows original + translated text, which provider translated it,
   and a copy button. Each translation is recorded to a local history.

The app runs in the system tray with a **Capture** / **Live Clipboard
Translate** / **Live Region Translate** / **History** / **Quit** menu.

## Installing

**Prebuilt release** (recommended if you don't want to build it yourself):
download the archive for your OS from the [Releases page](../../releases),
extract it, and run `ocr-translate` (`ocr-translate.exe` on Windows). Nothing
else to install — Tesseract's runtime DLLs (Windows) and English + Japanese
OCR data (matching the default config's `jpn+eng`) are already bundled
inside the archive. For a different OCR language, drop its `.traineddata`
file into the `tessdata` folder next to the binary (or set
`ocr.tessdata_dir`/`TESSDATA_PREFIX`) — see step 6 below for where to get one.
Linux still needs Tesseract + Leptonica installed via your distro's package
manager (see [Requirements](#requirements) below) — that part isn't bundled.

**From source**: see [Requirements](#requirements) below, then:

```sh
cargo build --release
```

## Requirements

### Linux

Only needed if you're building from source, except the Tesseract/Leptonica
runtime libraries (first bullet) — those are needed either way, prebuilt
binary or not, since only the OCR engine itself is a system dependency;
tessdata is bundled in the release archive (see [Installing](#installing)).

- Tesseract + Leptonica runtime libraries — dev packages if building from
  source (`tesseract`, `leptonica` on most distros)
- Rust toolchain
- X11 dev headers (`eframe`'s window backend keeps an X11 fallback compiled
  in; cursor position and monitor geometry are also queried over XWayland)
- PipeWire dev libraries (`libpipewire-0.3`, e.g. `pipewire-devel` /
  `libpipewire-0.3-dev`) — used by [Live Region Translate](#live-region-translate)
- GTK3 + libappindicator (or libayatana-appindicator) dev libraries, for the
  tray icon:
  ```sh
  # Arch / Manjaro
  sudo pacman -S gtk3 libappindicator-gtk3   # or libayatana-appindicator
  # Debian / Ubuntu
  sudo apt install libgtk-3-dev libappindicator3-dev  # or libayatana-appindicator3-dev
  ```

### Windows

Only needed if you're building from source — see [Installing](#installing)
above if you just want to run the app.

1. **Rust via [rustup](https://rustup.rs), MSVC ABI**:
   ```powershell
   rustup default stable-x86_64-pc-windows-msvc
   ```
   Don't install Rust via a plain `choco install rust` — that's a fixed
   `-gnu` toolchain with no ABI to switch. If you already have it installed
   alongside rustup, uninstall it (`choco uninstall rust`) and open a fresh
   terminal — Chocolatey's `bin` directory otherwise sits ahead of rustup's
   on `PATH` and silently wins.
2. **Visual Studio "Build Tools"**, with the "Desktop development with C++"
   workload — [download here](https://visualstudio.microsoft.com/visual-cpp-build-tools/).
   Needed for `link.exe`.
3. **Tesseract + Leptonica via [vcpkg](https://github.com/microsoft/vcpkg)**,
   with `VCPKG_ROOT` set:
   ```powershell
   git clone https://github.com/microsoft/vcpkg.git C:\vcpkg
   C:\vcpkg\bootstrap-vcpkg.bat
   C:\vcpkg\vcpkg.exe install tesseract:x64-windows
   [System.Environment]::SetEnvironmentVariable('VCPKG_ROOT', 'C:\vcpkg', 'User')
   [System.Environment]::SetEnvironmentVariable('VCPKGRS_DYNAMIC', 'true', 'User')
   ```
   Open a fresh terminal afterward. See `.github/workflows/autobuild.yml` for
   the equivalent CI setup.
4. **LLVM** (for `libclang`, used by `bindgen` to generate Tesseract/Leptonica
   bindings):
   ```powershell
   choco install llvm
   ```
   (or `winget install LLVM.LLVM`, or install manually and check "Add LLVM to
   the system PATH"). Open a fresh terminal afterward; if it's still not
   found, set `LIBCLANG_PATH` to wherever `libclang.dll` landed.
5. **Runtime DLLs**: Tesseract/Leptonica are linked dynamically
   (`VCPKGRS_DYNAMIC=true` above), so the built exe needs those DLLs next to
   it, not just at build time:
   ```powershell
   Copy-Item "$env:VCPKG_ROOT\installed\x64-windows\bin\*.dll" target\release\
   ```
   Packaged release archives already bundle these.
6. **`tessdata`**: vcpkg's `tesseract` port ships the OCR engine but not the
   trained language data. Download the `.traineddata` file(s) matching
   `ocr.languages` in your config (e.g. `eng.traineddata`) from
   `tesseract-ocr/tessdata_fast` on GitHub (or `tessdata`/`tessdata_best` for
   higher accuracy), then either drop them in a `tessdata` folder next to
   `target\release\ocr-translate.exe`, set `tessdata_dir` in `config.yaml` to
   wherever you put them, or set the `TESSDATA_PREFIX` environment variable.
   Packaged release archives already bundle English and Japanese (`eng`,
   `jpn`) — the default config's `jpn+eng`.

## Configuration

Config lives in a per-user directory named `ocr-translation`: `~/.config/ocr-translation`
on Linux, `%APPDATA%\ocr-translation` on Windows.

The first time you run the app, if no config exists yet, it creates that
directory with a ready-to-edit `config.yaml` plus reference copies
`config.example.yaml` and `config.example.conf`. Just start the app, then
edit `config.yaml`.

To reset back to the bundled defaults:

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
- `providers.<name>.kind` — `openai_compatible` | `google_translate` | `bing_translate` | `deepl_translate`.
- `providers.<name>.mode` — `public` | `private` (`google_translate`/`bing_translate`/`deepl_translate` only).
- `prompt.system` / `prompt.template` — customizable prompt sent to
  LLM-based providers. Placeholders: `{source_lang}`, `{target_lang}`, `{text}`.
- `ocr.languages` — Tesseract language code(s), e.g. `eng`, `eng+jpn`.
- `capture.backend` — `built_in` | `external` (see [Alternate capture backend](#alternate-capture-backend-external-tool)).
- `history.enabled` / `history.max_entries` — see [History](#history).
- `popup` — sizes the crop-selector window (`capture` and `watch-region`'s
  region pick); `translate` — sizes every translation-result window
  (`capture`, Live Clipboard/Region Translate); `history_popup` — sizes the
  history-entry viewer. See [Popup, translate, and history windows](#popup-translate-and-history-windows).

API keys: prefer `api_key_env: SOME_ENV_VAR` over a literal `api_key` in the
file, so secrets don't end up on disk in plaintext.

## Providers, fallback, and public/private modes

`active_provider` is tried first; if it fails (server not running, bad key,
network error, ...) each entry in `fallback_providers` is tried next, in
order, until one succeeds:

```yaml
active_provider: lmstudio
fallback_providers:
  - google
```

Google Translate, Bing/Azure Translator, and DeepL Translator each support two `mode`s:

- `mode: public` — a free, unofficial, no-key endpoint the provider's own
  translator web page uses. Undocumented, and can change or rate-limit
  without notice.
- `mode: private` — the official, authenticated API (Cloud Translation v2 for
  Google, Azure Translator v3 for Bing, the DeepL API — Free or Pro tier,
  detected from whether the key ends in `:fx`). Needs `api_key`/`api_key_env`
  (and `region` for Bing, if using a multi-service Azure resource).

Public mode never sends `prompt.system`/`prompt.template` — these are real
translation APIs, not LLMs.

DeepSeek (an LLM API, not a dedicated translation service — not to be
confused with DeepL above) has no free/public option — it always requires
`api_key_env`.

## Alternate capture backend (external tool)

Instead of the built-in grab-and-crop flow, `capture.backend: external` runs
a configurable shell command and reads the image it leaves on the clipboard,
skipping the crop window entirely — for external tools that do their own
**live** region selection on the real desktop. For example, on KDE:

```yaml
capture:
  backend: external
  external_command: "spectacle -r -b -c"
  external_timeout_secs: 10
```

Other examples: `grim -g "$(slurp)" - | wl-copy` (wlroots compositors),
`gnome-screenshot -a -c` (GNOME). Not the default, since it depends on a
specific tool being installed, but a nicer selection experience where
available.

## History

Each translation is recorded to `history.jsonl` (JSON Lines) next to your
config file. Controlled by `history.enabled`, `history.max_entries`,
`history.tray_menu_entries`.

The tray's **History** submenu lists recent entries; click one to reopen it
in the popup, or use **Clear History**. Also available from the CLI:

```sh
ocr-translate show-history 0    # 0 = most recent
ocr-translate clear-history
```

Reopened entries use the separate `history_popup` config section — see
[Popup, translate, and history windows](#popup-translate-and-history-windows).

## Popup, translate, and history windows

Three config sections control every window this app opens, split by what
they show:

- **`popup`** — the crop-selector window, "drag a rectangle to select."
  Shared by `capture` and `watch-region`'s initial region pick. Fields:
  `width`, `height` (the box the screenshot is scaled to fit within, aspect
  ratio preserved — scroll to zoom further), `always_on_top`.
- **`translate`** — every window that shows a translation result: the
  one-shot `capture` popup, Live Clipboard Translate, and Live Region
  Translate. Fields: `width`, `height`, `font_size`, `always_on_top`,
  `auto_close_secs`. `auto_close_secs` only applies to the one-shot
  `capture` popup — the live windows are meant to keep running until you
  close them yourself.
- **`history_popup`** — the window shown when reopening a history entry.
  Same shape as `translate`, kept separate so you can size it differently.

## Live Clipboard Translate

Watches the clipboard: copy some text anywhere, and a popup shows its
translation within `live_translate.poll_interval_ms` (default 0.5s). Copy
something else and the same popup updates in place. Never recorded to
history.

Start it from the tray's **Live Clipboard Translate** menu item, or:

```sh
ocr-translate watch-clipboard
```

The popup has a **Show source** checkbox (initial state from
`live_translate.show_source_by_default`). Sizing/appearance comes from the
shared `translate` config section.

## Live Region Translate

Like Live Clipboard Translate, but for a fixed screen *region* — useful for
subtitles, a chat window, a status readout, or anything else that updates in
place on screen.

1. Starts a continuous screen capture session (Linux: a
   `org.freedesktop.portal.ScreenCast` session via PipeWire — your
   compositor will ask you to pick a screen/window to share; Windows:
   `xcap`'s DXGI-based screen recording, no picker needed).
2. Waits `region_translate.capture_delay_secs` (default 1s) before grabbing
   the frame you'll select a region on.
3. Shows that frame in the same zoom/pan/select window used by `capture` —
   drag a rectangle around the text you want to watch.
4. From then on, polls every `region_translate.poll_interval_ms` (default
   1s), OCRs just that rectangle, and re-translates only when the
   recognized text changes.

Never recorded to history. Start it from the tray's **Live Region
Translate** menu item, or:

```sh
ocr-translate watch-region
```

Sizing/appearance comes from the shared `translate` config section;
`region_translate` itself only holds `show_source_by_default`,
`poll_interval_ms`, `capture_delay_secs`. The region-selection window uses
the `popup` section.

On Linux this needs a working ScreenCast portal backend
(`xdg-desktop-portal-kde`, `-gnome`, `-wlr`, or `-hyprland`) and PipeWire —
standard on any desktop that supports screen sharing in a video call. On
Windows, no extra setup beyond [Requirements](#requirements) above.

## Config hot-reload

While `ocr-translate run` is active, editing the config file takes effect
without restarting — a background watcher polls it every ~2 seconds:

- Provider/prompt/OCR/popup/capture/history settings apply the next time you
  trigger a fresh capture or Live Clipboard/Region Translate window. An
  already-open Live Clipboard/Region Translate window keeps its original
  settings until closed and reopened.
- The tray's History submenu settings (`tray_menu_entries`) apply on its
  next refresh.
- A config file that fails to parse mid-edit is ignored — the previous valid
  config keeps running.

## Running

```sh
# Tray daemon (default): sits in the system tray with a
# Capture / Live Clipboard Translate / Live Region Translate / History / Quit menu.
ocr-translate run

# One-shot: capture, crop, OCR, translate, show popup, exit.
ocr-translate capture

# Watch the clipboard and show a live-updating translation popup.
ocr-translate watch-clipboard

# Pick a screen region and show a live-updating translation popup.
ocr-translate watch-region

# Sanity-check a provider without touching the screen:
ocr-translate test-provider --provider openai "Bonjour le monde"
```

`ocr-translate run` only ever runs one at a time — starting a second one
while the tray daemon is already running shows an error popup and exits
rather than opening a duplicate tray icon. `capture`/`watch-clipboard`/
`watch-region`/`show-history` aren't limited this way — running several at
once (a capture while Live Region Translate is open, two captures back to
back, etc.) is fine.

### Binding a key

There's no in-app global hotkey. Bind `ocr-translate capture` to a key
yourself, natively in your DE/compositor:

- **KDE Plasma**: System Settings → Shortcuts → add a custom command
  shortcut that runs `ocr-translate capture`.
- **GNOME**: Settings → Keyboard → Keyboard Shortcuts → Custom Shortcuts.
- **Sway / Hyprland / i3**: bind it in your compositor config, e.g. for Sway:
  ```
  bindsym $mod+Shift+o exec ocr-translate capture
  ```
- **Windows**: bind it via a shortcut key on a Start Menu/desktop shortcut,
  or a third-party hotkey tool.

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

## FAQ

**Why no global hotkey?**
The only Wayland mechanism for one, the `org.freedesktop.portal.GlobalShortcuts`
portal, only grants shortcuts to Flatpak/Snap-sandboxed apps in practice — a
plain binary gets `NotAllowed: An app id is required`. Bind
`ocr-translate capture` to a key in your DE/compositor instead (see
[Binding a key](#binding-a-key)) — it goes through the compositor's own
shortcut system directly, so it always works.

**Why isn't macOS supported?**
Not implemented yet.

**Why is Live Region Translate/`watch-region` different on Windows vs Linux?**
Linux has no portable "overlay the live desktop" API, and no single
capture mechanism that works the same way across compositors for a
continuous feed, so this uses the `ScreenCast` xdg-desktop-portal (the same
one video-calling apps use). Windows uses `xcap`'s screen-recording API
(DXGI Desktop Duplication) instead, which needs no user picker dialog.

**Does capture work on X11?**
No — Linux support targets Wayland only (GNOME/KDE/wlroots). Cursor position
and monitor geometry are queried over XWayland as an implementation detail
(Wayland doesn't expose either to arbitrary clients), but this isn't X11
session support.

**Which provider actually translated my text?**
The popup shows `via <provider>` under the translated text — useful with a
fallback chain configured, since it isn't always `active_provider`.

**Where's my history stored, and how do I clear it?**
`history.jsonl` next to your config file. Use the tray's **Clear History**,
or `ocr-translate clear-history`.

**A window opened at the wrong size.**
Window sizes are clamped a little short of your exact monitor resolution to
avoid a Wayland/KWin windowing bug where a window landing within a few
pixels of the monitor's exact resolution collapses to a tiny fallback size.
This is automatic — just set the size you actually want.

**The `external` capture backend says "cancelled" but I did select something.**
It detects a fresh capture by diffing the clipboard image against what was
there before the command ran, so capturing the exact same pixels twice in a
row reads as cancelled the second time.

**(Windows) A window fails to open with `egui_glow: OpenGL: egui_glow requires opengl 2.0+`.**
Your display driver doesn't support hardware OpenGL — common in a VM or
remote desktop session without GPU passthrough, uncommon on real hardware
with a normal GPU driver installed. Fix: download a prebuilt software
OpenGL implementation for Windows (e.g. the `pal1000/mesa-dist-win` project
on GitHub) and copy its `opengl32.dll` and `libgallium_wgl.dll` into the
same folder as `ocr-translate.exe`.

---

This project was built with the help of AI (Claude Code).
