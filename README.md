# ocr-translate

A small Rust tool for Linux: press a hotkey (or use the tray menu), drag-select
a region of the screen, and get the OCR'd text translated by an LLM or
translation API in a popup. Built to work across X11 and Wayland
(GNOME/KDE/wlroots) without compositor-specific hacks, and the codebase is
otherwise plain Rust so it also builds on Windows/macOS with reduced
hotkey/portal/tray support.

## How it works

1. **Capture**: grabs a screenshot of just the monitor the cursor is
   currently on (so a multi-monitor setup never hands OCR a giant combined
   image) — via the `org.freedesktop.portal.Screenshot` xdg-desktop-portal on
   Wayland, or a direct X11 grab otherwise.
2. **Select**: shows that screenshot in a borderless window; scroll to zoom,
   right-drag to pan, and left-drag a rectangle to crop. This sidesteps the
   fact that neither X11 nor Wayland has one portable "overlay the live
   desktop" API, and zoom makes it practical to select small text precisely.
3. **OCR**: runs the crop through Tesseract (via `leptess`).
4. **Translate**: sends the recognized text to a configured backend — an
   OpenAI-compatible chat API (LM Studio, Ollama, OpenAI, DeepSeek, ...),
   Google Cloud Translation, or Microsoft/Azure ("Bing") Translator.
5. **Popup**: shows original + translated text, with a copy button.

The app runs in the system tray with a simple **Capture** / **Quit** menu, in
addition to whichever hotkey mechanism your desktop supports (see below).

## Requirements

- Rust toolchain
- Tesseract + Leptonica dev libraries (`tesseract`, `leptonica` on most distros)
  and at least one language's tessdata installed
- X11 dev headers (only needed for the X11 capture/hotkey fallback path)
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

- `active_provider` — which entry in `providers` to use.
- `providers.<name>.kind` — `openai_compatible` | `google_translate` | `bing_translate`.
- `prompt.system` / `prompt.template` — fully customizable prompt sent to
  LLM-based providers. Template placeholders: `{source_lang}`, `{target_lang}`, `{text}`.
- `ocr.languages` — Tesseract language code(s), e.g. `eng`, `eng+jpn`.
- `hotkey.capture_region` — accelerator string, e.g. `CTRL+ALT+O`.

API keys: prefer `api_key_env: SOME_ENV_VAR` over a literal `api_key` in the
file, so secrets don't end up on disk in plaintext.

## Running

```sh
# Tray + hotkey daemon (default): sits in the system tray with a
# Capture / Quit menu, and also listens for the configured hotkey.
ocr-translate run

# One-shot: capture, crop, OCR, translate, show popup, exit. Useful for
# binding to a key yourself (see below) or for scripting.
ocr-translate capture

# Sanity-check a provider without touching the screen:
ocr-translate test-provider --provider openai "Bonjour le monde"
```

### Hotkey behavior differs by session type

Regardless of which of these applies, the tray's **Capture** menu item always
works, and running `ocr-translate capture` always works too.

- **X11** (including XWayland): the daemon grabs `hotkey.capture_region`
  directly. Works out of the box.
- **Wayland, GNOME 43+ / KDE Plasma 6+**: the daemon requests the shortcut
  through the `GlobalShortcuts` desktop portal. In practice this portal grants
  shortcuts to Flatpak/Snap-sandboxed apps; a plain binary run from a terminal
  gets `NotAllowed: An app id is required` — and, confirmed by testing, wrapping
  the process in a matching systemd `app-<id>-<random>.scope`/`.service` (the
  naming convention the portal's own app-id detection looks for) does **not**
  help either, so this isn't something we can work around from inside the app
  short of Flatpak-packaging it. Use one of the options below instead.
- **Wayland, Sway / Hyprland / i3 / anything without that portal**: no app can
  grab a truly global hotkey either.

Whichever of the above applies, two things always work: the tray's **Capture**
menu item, and binding a key yourself to run `ocr-translate capture`:

- **KDE Plasma**: System Settings → Shortcuts → add a custom command shortcut
  that runs `ocr-translate capture`. This uses KWin's native global shortcut
  system directly and does not go through the portal at all.
- **Sway / Hyprland / i3**: bind it in your compositor config, e.g. for Sway:
  ```
  bindsym $mod+Shift+o exec ocr-translate capture
  ```
- **GNOME**: Settings → Keyboard → Keyboard Shortcuts → Custom Shortcuts.

### Running as a systemd user service

```ini
# ~/.config/systemd/user/ocr-translate.service
[Unit]
Description=ocr-translate tray + hotkey daemon

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
- The `GlobalShortcuts` portal effectively requires Flatpak/Snap sandboxing in
  practice; for a plain binary, use the tray menu or bind `ocr-translate
  capture` natively in your DE/WM instead (see above).
- Active-monitor detection queries the pointer position over X11/XWayland; on
  a pure-Wayland session with no XWayland at all, it falls back to the
  primary monitor (X11 backend) or the full multi-monitor screenshot (portal
  backend), rather than failing.
