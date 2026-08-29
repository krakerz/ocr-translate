# ocr-translate

A small Rust tool for **Linux (Wayland) and Windows**: use the tray menu (or
bind `ocr-translate capture` to a key yourself), drag-select a region of the
screen, and get the OCR'd text translated by an LLM or translation API in a
popup. **macOS isn't supported yet.**

On Linux this targets Wayland (GNOME/KDE/wlroots) via the standard
xdg-desktop-portal APIs, without compositor-specific hacks; X11 sessions
aren't supported as a first-class target there — see [Known
limitations](#known-limitations) for the one place X11/XWayland is still
used, as an implementation detail. This is the mature, day-to-day-verified
platform.

On Windows, one-shot capture (via `xcap`) is now **confirmed working
end-to-end** — screenshot, OCR, and translation all verified producing
correct real output on a real (virtualized) Windows 11 machine — though it's
newer than the Linux support and hasn't been tried on genuine physical
hardware yet. Live Region Translate isn't implemented on Windows yet (a
clear error rather than a crash, if you try `watch-region` there). See the
Windows [Requirements](#requirements) below for the build-environment setup
this needed, and `TODO.md` (untracked, local) for the full verification
status if you're working on this.

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
5. **Popup**: shows original + translated text, which provider actually
   translated it (`via google`, `via deepl`, ...) — useful when you have a
   fallback chain configured, since it's not always `active_provider` — and a
   copy button. Each successful translation is also recorded to a local
   history — see [History](#history).

The app runs in the system tray with a **Capture** / **Live Clipboard
Translate** / **Live Region Translate** / **History** / **Quit** menu (see
[Live Clipboard Translate](#live-clipboard-translate) for the clipboard-based
mode, and [Live Region Translate](#live-region-translate) for the fixed-region
mode). There's no built-in global hotkey — see [Binding a key](#binding-a-key)
for why, and how to bind one yourself in your DE/compositor instead.

## Requirements

### Linux

- Rust toolchain
- Tesseract + Leptonica dev libraries (`tesseract`, `leptonica` on most distros)
  and at least one language's tessdata installed
- X11 dev headers — not for capture (that's portal-only now), but `eframe`'s
  window backend keeps an X11 fallback compiled in alongside its Wayland
  backend; also, cursor position and monitor geometry are queried over
  XWayland (see [Known limitations](#known-limitations))
- PipeWire dev libraries (`libpipewire-0.3`, e.g. `pipewire-devel` /
  `libpipewire-0.3-dev` depending on distro) — used only by
  [Live Region Translate](#live-region-translate)'s ScreenCast capture; any
  desktop that already supports screen sharing in a video call has these
- GTK3 + libappindicator (or libayatana-appindicator) dev libraries, for the
  tray icon:
  ```sh
  # Arch / Manjaro
  sudo pacman -S gtk3 libappindicator-gtk3   # or libayatana-appindicator
  # Debian / Ubuntu
  sudo apt install libgtk-3-dev libappindicator3-dev  # or libayatana-appindicator3-dev
  ```

### Windows (confirmed working end-to-end on a VM — see the note above; not yet tried on physical hardware)

Every step below was hit and confirmed by testing on a real Windows machine
while getting this working, in the order they tend to come up:

1. **Rust toolchain — use `rustup`, and the `-msvc` ABI, not `-gnu`.**
   Install via [rustup](https://rustup.rs) directly, or `choco install
   rustup.install` if you're on Chocolatey (**not** the plain `choco install
   rust` package — that's a fixed toolchain snapshot with no ABI to switch,
   unlike rustup, and its binaries are `-gnu`). Then:
   ```powershell
   rustup default stable-x86_64-pc-windows-msvc
   ```
   **If you already had `choco install rust` installed, uninstall it —
   don't just add rustup alongside it**: Chocolatey's `bin` directory sits
   on the system-wide `PATH` ahead of rustup's user-level one, so its
   `cargo.exe`/`rustc.exe` silently win every invocation regardless of what
   `rustup default`/`rustup show` report (confirmed by testing: `rustup
   show` reported `msvc` as active while `rustc --print cfg` still showed
   `target_env="gnu"`, traced to `Get-Command cargo -All` listing
   `C:\ProgramData\chocolatey\bin\cargo.exe` ahead of
   `%USERPROFILE%\.cargo\bin\cargo.exe`). Run `choco uninstall rust`, open a
   **new** terminal (PATH changes don't apply to already-open ones), and
   confirm with `Get-Command cargo -All` / `rustc --print cfg` before
   moving on — `rustup show` alone isn't enough to trust here.
2. **Visual Studio "Build Tools" with the "Desktop development with C++"
   workload** — [download here](https://visualstudio.microsoft.com/visual-cpp-build-tools/).
   The MSVC ABI needs `link.exe` from this; without it you'll hit `error:
   linker `link.exe` not found`. Installing just VS Code isn't sufficient
   (the compiler error itself says so).
3. **Tesseract + Leptonica via [vcpkg](https://github.com/microsoft/vcpkg)**,
   plus **`VCPKG_ROOT` actually set** — confirmed by testing that without
   it, the build fails with `VcpkgNotFound("No vcpkg installation
   found...")` even with vcpkg itself installed and working. `C:\vcpkg`
   below is just an example location — clone it wherever you like and point
   `VCPKG_ROOT` at that instead:
   ```powershell
   git clone https://github.com/microsoft/vcpkg.git C:\vcpkg
   C:\vcpkg\bootstrap-vcpkg.bat
   C:\vcpkg\vcpkg.exe install tesseract:x64-windows
   [System.Environment]::SetEnvironmentVariable('VCPKG_ROOT', 'C:\vcpkg', 'User')
   [System.Environment]::SetEnvironmentVariable('VCPKGRS_DYNAMIC', 'true', 'User')
   ```
   Open a new terminal after setting those (same PATH-refresh reason as
   step 1), then build. See `.github/workflows/autobuild.yml` for the equivalent
   CI setup (that runner ships vcpkg pre-installed, so it only needs the
   `install` + `VCPKG_ROOT` steps, not the clone/bootstrap).
4. **LLVM (for `libclang`)** — `leptonica-sys`/`tesseract-sys` use `bindgen`
   to generate Rust bindings from Tesseract/Leptonica's C headers, which
   needs `libclang.dll` at build time. Confirmed by testing: without it, the
   build fails with `Unable to find libclang: "couldn't find any valid
   shared libraries matching: ['clang.dll', 'libclang.dll']..."` — even
   though vcpkg itself found Tesseract/Leptonica correctly by that point.
   ```powershell
   choco install llvm
   ```
   (or `winget install LLVM.LLVM`, or the installer directly from
   [LLVM's releases](https://github.com/llvm/llvm-project/releases) — check
   "Add LLVM to the system PATH" if it's offered). Open a new terminal
   afterward; if it's still not found, set `LIBCLANG_PATH` explicitly to
   wherever `libclang.dll` landed (typically `C:\Program Files\LLVM\bin`).
   **CI doesn't need this step** — GitHub's `windows-latest` runner ships
   LLVM pre-installed (confirmed via its
   [runner-images software manifest](https://github.com/actions/runner-images/blob/main/images/windows/Windows2022-Readme.md)),
   so this is purely a local-machine setup gap.
5. **After a successful build, running the exe can still fail** with `The
   code execution cannot proceed because tesseract55.dll was not found.
   Reinstalling the program may fix this problem.` — confirmed by testing.
   Step 3's `VCPKGRS_DYNAMIC=true` links Tesseract/Leptonica dynamically, so
   the exe needs `tesseract55.dll` and its dependencies (Leptonica, libpng,
   zlib, libjpeg-turbo, libwebp, libtiff, openjp2, etc.) discoverable at
   runtime, not just at link time — vcpkg builds these but doesn't put them
   on PATH or next to your exe for you. Fix: copy everything from
   `%VCPKG_ROOT%\installed\x64-windows\bin\` next to
   `target\release\ocr-translate.exe` (or add that directory to PATH):
   ```powershell
   Copy-Item "$env:VCPKG_ROOT\installed\x64-windows\bin\*.dll" target\release\
   ```
   The packaged release archives from `autobuild.yml` already bundle these
   DLLs alongside the exe, so this step is only needed for a local dev build.
6. **`tessdata` (Tesseract's trained language data) isn't installed by
   anything above.** Confirmed by testing: without it, `capture`/etc. fail
   with `failed to initialize Tesseract (is 'tessdata' for the configured
   language installed?): TessInitError{-1}`. vcpkg's `tesseract` port only
   ships the OCR engine library, not the trained data — same as Linux, which
   also needs "at least one language's tessdata installed" separately (see
   Linux requirements above), just via a distro package there instead.
   Download the `.traineddata` file(s) matching `ocr.languages` in your
   config (e.g. `eng.traineddata`) from the official `tesseract-ocr/tessdata_fast`
   GitHub repo (smaller/faster models — the usual choice; `tessdata`/
   `tessdata_best` if you want higher accuracy instead), put them in a
   folder, then either set `tessdata_dir` in `config.yaml` to that folder or
   set the `TESSDATA_PREFIX` environment variable to it.
- Everything else (capture, tray, translate, popups) is either genuinely
  cross-platform already or has a Windows-specific backend — no other native
  library requirement beyond Tesseract/Leptonica right now. Live Region
  Translate (`watch-region`) isn't implemented on Windows yet.

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
- `providers.<name>.kind` — `openai_compatible` | `google_translate` | `bing_translate` | `deepl_translate`.
- `providers.<name>.mode` — `public` | `private` (`google_translate`/`bing_translate`/`deepl_translate` only).
- `prompt.system` / `prompt.template` — fully customizable prompt sent to
  LLM-based providers. Template placeholders: `{source_lang}`, `{target_lang}`, `{text}`.
- `ocr.languages` — Tesseract language code(s), e.g. `eng`, `eng+jpn`.
- `capture.backend` — `built_in` | `external` (see [Alternate capture backend](#alternate-capture-backend-external-tool)).
- `history.enabled` / `history.max_entries` — see [History](#history).
- `popup` — sizes the in-app crop-selector window (used by `capture` and `watch-region`'s region pick), `translate` — sizes every window that shows a translation result (`capture`, Live Clipboard/Region Translate), `history_popup` — sizes the history-entry viewer. See [Popup, translate, and history windows](#popup-translate-and-history-windows).

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

Google Translate, Bing/Azure Translator, and DeepL Translator each support two `mode`s:

- `mode: public` — a free, unofficial, no-key endpoint the provider's own
  translator web page uses: load the page once to pick up a session
  token/cookies (the same technique [XUnity.AutoTranslator](https://github.com/bbepis/XUnity.AutoTranslator)
  uses for its free endpoints, and that this project's own implementation is
  modeled on), then reuse that session across translations, refreshed
  periodically rather than on every call. Undocumented, and can change or
  rate-limit without notice:
  - **Google**: works reliably in testing.
  - **Bing**: works, but needed one more piece than XUnity's own
    implementation has — confirmed by testing that XUnity's exact request
    gets rejected by the live endpoint today (`{"statusCode":205}`); the
    missing piece was an additional auth token/key the page embeds
    (`params_AbusePreventionHelper`), which this implementation scrapes and
    sends too.
  - **DeepL**: implemented the same way, but hit a 429 (rate-limited) on the
    first live attempt in testing — treat this as the least reliable of the
    three, and prefer `mode: private` if you have a DeepL API key.
- `mode: private` — the official, authenticated API (Cloud Translation v2 for
  Google, Azure Translator v3 for Bing, the DeepL API — Free or Pro tier,
  detected from whether the key ends in `:fx` — for DeepL). Needs
  `api_key`/`api_key_env` (and `region` for Bing, if using a multi-service
  Azure resource).

Public mode never sends `prompt.system`/`prompt.template` — these are real
translation APIs, not LLMs, so there's no prompt to customize.

DeepSeek (an LLM API, not a dedicated translation service — not to be
confused with DeepL above) has no free/public option either — it always
requires `api_key_env`.

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
(width/height/font_size/always_on_top/auto_close_secs) — see
[Popup, translate, and history windows](#popup-translate-and-history-windows) —
so you can size the history viewer differently than the live capture-result
popup, e.g. larger for comfortably re-reading older entries.

## Popup, translate, and history windows

Three config sections control every window this app opens, split by *what
they show* rather than by which command triggered them:

- **`popup`** — the in-app crop-selector window, "drag a rectangle to
  select." Shared by `ocr-translate capture` (`built_in` backend) and
  `watch-region`'s initial region pick — both are "here's a screenshot, draw
  a box on it" moments, so they share one config. Fields: `width`, `height`
  (the box the screenshot is initially scaled to fit within, aspect ratio
  preserved — scroll to zoom in further from there; doesn't need to match
  your monitor's resolution), `always_on_top`.
- **`translate`** — every window that shows a translation result: the
  one-shot `capture` popup, Live Clipboard Translate, and Live Region
  Translate. Fields: `width`, `height`, `font_size`, `always_on_top`,
  `auto_close_secs`. **`auto_close_secs` only applies to the one-shot
  `capture` popup** — Live Clipboard/Region Translate ignore it, since those
  windows are meant to keep running (and updating in place) until you close
  them yourself; setting it wouldn't make sense there.
- **`history_popup`** — the window shown when reopening a history entry
  (tray History submenu / `ocr-translate show-history`). Same shape as
  `translate`, kept as its own section so you can size it differently, e.g.
  larger for comfortably re-reading old entries.

**A note on setting `width`/`height` to your full monitor resolution**: this
works, but is automatically clamped a little short of the exact number —
requesting a window size that lands within a few pixels of your monitor's
actual resolution triggers a confirmed Wayland/KWin windowing bug where the
window collapses to a tiny fallback size instead of the one you asked for.
All three sections above apply this clamp automatically, so you don't need
to work around it yourself — just set the size you actually want (even your
exact screen resolution) and it'll open as close to that as the workaround
allows.

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
comes from `live_translate.show_source_by_default`. Sizing/appearance comes
from the shared `translate` config section (see
[Popup, translate, and history windows](#popup-translate-and-history-windows));
`live_translate` itself only holds `show_source_by_default` and
`poll_interval_ms`.

## Live Region Translate

Like Live Clipboard Translate, but for a fixed screen *region* instead of the
clipboard — useful for subtitles, a chat window, a status readout, or
anything else that updates in place on screen. Starting it:

1. Opens a `org.freedesktop.portal.ScreenCast` session via PipeWire — your
   compositor will ask you to pick a screen/window to share (KDE's picker
   looks like a small "Share screen with..." dialog). This happens once per
   run, the same portal used by conferencing/recording apps.
2. Waits `region_translate.capture_delay_secs` (default 1s) before grabbing
   the frame you'll select a region on — the picker dialog itself can still
   be visible in the very first frames, so this gives it time to actually
   close first. Raise this if you still see the dialog baked into the
   screenshot in step 3 (a slower compositor may need more than 1s).
3. Shows the first captured frame in the same zoom/pan/select window used by
   `capture` — drag a rectangle around the text you want to watch.
4. From then on, polls the live ScreenCast stream every
   `region_translate.poll_interval_ms` (default 1s), OCRs just that
   rectangle, and re-translates only when the recognized text actually
   changes (not on every poll) — so a static region doesn't get retranslated
   repeatedly, and a subtitle track gets picked up as soon as the line
   changes.

Deliberately **never recorded to history**, matching Live Clipboard
Translate. Start it from the tray's **Live Region Translate** menu item, or:

```sh
ocr-translate watch-region
```

Sizing/appearance comes from the shared `translate` config section (see
[Popup, translate, and history windows](#popup-translate-and-history-windows));
`region_translate` itself only holds `show_source_by_default`,
`poll_interval_ms`, and `capture_delay_secs`. The region-selection window in
step 3 uses the `popup` section, same as `capture`'s. OCR is much more
expensive than the clipboard's plain text-change check, so its default
`poll_interval_ms` is slower (1s vs 0.5s) — lower it if you need snappier
updates and can afford the CPU, e.g. for fast-moving subtitles.

Requires a working ScreenCast portal backend (`xdg-desktop-portal-kde`,
`-gnome`, `-wlr`, or `-hyprland` depending on your compositor) and PipeWire
running, which is standard on any desktop that already supports screen
sharing in a video call or `OBS`/`wf-recorder`-style recording.

## Config hot-reload

While `ocr-translate run` is active, editing the config file takes effect
without restarting it — a background watcher polls it every ~2 seconds and
reloads on change:

- Anything used by a fresh capture or a new Live Clipboard/Region Translate
  window (providers, prompt, OCR, popup sizes, capture backend, history
  settings, `live_translate.*`, `region_translate.*`, ...) applies the next
  time you trigger one — each runs in its own process that reads the config
  file fresh, so this was already true before hot-reload existed. An
  *already-open* Live Clipboard/Region Translate window keeps using the
  settings it started with until you close and reopen it.
- The tray's History submenu settings (`tray_menu_entries`) apply on its next
  refresh (a couple of seconds), no restart needed.
- A config file that fails to parse while being edited (e.g. mid-save) is
  logged and ignored — the previous valid config keeps running rather than
  crashing the daemon.

## Running

```sh
# Tray daemon (default): sits in the system tray with a
# Capture / Live Clipboard Translate / Live Region Translate / History / Quit menu.
ocr-translate run

# One-shot: capture, crop, OCR, translate, show popup, exit. Useful for
# binding to a key yourself (see below) or for scripting.
ocr-translate capture

# Watch the clipboard and show a live-updating translation popup (see
# Live Clipboard Translate above). Never recorded to history.
ocr-translate watch-clipboard

# Pick a screen region (via PipeWire ScreenCast) and show a live-updating
# translation popup (see Live Region Translate above). Never recorded to history.
ocr-translate watch-region

# Sanity-check a provider without touching the screen:
ocr-translate test-provider --provider openai "Bonjour le monde"
```

`ocr-translate run` only ever runs one at a time — if you (or an autostart
entry) start a second one while the tray daemon is already running, it shows
an error popup and exits rather than opening a duplicate tray icon. This
doesn't affect `capture`/`watch-clipboard`/`watch-region`/`show-history`:
those are the commands your hotkey binding actually runs (see below), and
running several of them at once — a capture while Live Region Translate is
open, two captures back to back, etc. — is completely fine.

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
