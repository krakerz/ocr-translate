use std::sync::mpsc::Sender;
use std::sync::Mutex;

use anyhow::Result;
use global_hotkey::hotkey::HotKey;
use global_hotkey::GlobalHotKeyManager;

use crate::daemon::DaemonEvent;

pub(crate) fn is_wayland() -> bool {
    std::env::var("WAYLAND_DISPLAY").is_ok()
        || std::env::var("XDG_SESSION_TYPE").as_deref() == Ok("wayland")
}

/// Lets the config watcher re-bind the X11 hotkey live, without restarting
/// the daemon, when `hotkey.capture_region` changes on disk.
pub(crate) struct X11HotkeyHandle {
    manager: &'static GlobalHotKeyManager,
    current: Mutex<HotKey>,
}

impl X11HotkeyHandle {
    /// No-ops if `accelerator` parses to the same hotkey already registered.
    pub(crate) fn update(&self, accelerator: &str) -> Result<()> {
        let new_hotkey = parse_accelerator(accelerator)?;
        let mut current = self.current.lock().unwrap();
        if *current == new_hotkey {
            return Ok(());
        }
        self.manager.unregister(*current)?;
        self.manager.register(new_hotkey)?;
        *current = new_hotkey;
        Ok(())
    }
}

pub(crate) fn spawn_x11_listener(
    tx: Sender<DaemonEvent>,
    accelerator: &str,
) -> Result<X11HotkeyHandle> {
    use global_hotkey::{GlobalHotKeyEvent, HotKeyState};

    let hotkey = parse_accelerator(accelerator)?;
    // Leaked intentionally: the manager must live for the process lifetime.
    let manager: &'static GlobalHotKeyManager = Box::leak(Box::new(GlobalHotKeyManager::new()?));
    manager.register(hotkey)?;

    std::thread::spawn(move || {
        let receiver = GlobalHotKeyEvent::receiver();
        loop {
            if let Ok(event) = receiver.recv() {
                if event.state == HotKeyState::Pressed {
                    let _ = tx.send(DaemonEvent::Capture);
                }
            }
        }
    });
    Ok(X11HotkeyHandle {
        manager,
        current: Mutex::new(hotkey),
    })
}

fn parse_accelerator(accelerator: &str) -> Result<global_hotkey::hotkey::HotKey> {
    use global_hotkey::hotkey::{Code, HotKey, Modifiers};

    let mut modifiers = Modifiers::empty();
    let mut code: Option<Code> = None;
    for part in accelerator.split(['+', '-']) {
        let part = part.trim();
        match part.to_uppercase().as_str() {
            "CTRL" | "CONTROL" => modifiers |= Modifiers::CONTROL,
            "ALT" => modifiers |= Modifiers::ALT,
            "SHIFT" => modifiers |= Modifiers::SHIFT,
            "SUPER" | "META" | "CMD" | "WIN" => modifiers |= Modifiers::SUPER,
            key => code = Some(code_from_str(key)?),
        }
    }
    let code =
        code.ok_or_else(|| anyhow::anyhow!("hotkey '{accelerator}' has no non-modifier key"))?;
    Ok(HotKey::new(Some(modifiers), code))
}

fn code_from_str(key: &str) -> Result<global_hotkey::hotkey::Code> {
    use global_hotkey::hotkey::Code;
    if key.len() == 1 {
        let c = key.chars().next().unwrap();
        if c.is_ascii_alphabetic() {
            let idx = c.to_ascii_uppercase() as u8 - b'A';
            let letters = [
                Code::KeyA,
                Code::KeyB,
                Code::KeyC,
                Code::KeyD,
                Code::KeyE,
                Code::KeyF,
                Code::KeyG,
                Code::KeyH,
                Code::KeyI,
                Code::KeyJ,
                Code::KeyK,
                Code::KeyL,
                Code::KeyM,
                Code::KeyN,
                Code::KeyO,
                Code::KeyP,
                Code::KeyQ,
                Code::KeyR,
                Code::KeyS,
                Code::KeyT,
                Code::KeyU,
                Code::KeyV,
                Code::KeyW,
                Code::KeyX,
                Code::KeyY,
                Code::KeyZ,
            ];
            return Ok(letters[idx as usize]);
        }
        if c.is_ascii_digit() {
            let idx = c as u8 - b'0';
            let digits = [
                Code::Digit0,
                Code::Digit1,
                Code::Digit2,
                Code::Digit3,
                Code::Digit4,
                Code::Digit5,
                Code::Digit6,
                Code::Digit7,
                Code::Digit8,
                Code::Digit9,
            ];
            return Ok(digits[idx as usize]);
        }
    }
    match key {
        "SPACE" => Ok(Code::Space),
        "ENTER" | "RETURN" => Ok(Code::Enter),
        "TAB" => Ok(Code::Tab),
        "ESCAPE" | "ESC" => Ok(Code::Escape),
        "F1" => Ok(Code::F1),
        "F2" => Ok(Code::F2),
        "F3" => Ok(Code::F3),
        "F4" => Ok(Code::F4),
        "F5" => Ok(Code::F5),
        "F6" => Ok(Code::F6),
        "F7" => Ok(Code::F7),
        "F8" => Ok(Code::F8),
        "F9" => Ok(Code::F9),
        "F10" => Ok(Code::F10),
        "F11" => Ok(Code::F11),
        "F12" => Ok(Code::F12),
        other => Err(anyhow::anyhow!(
            "unrecognized key '{other}' in hotkey accelerator"
        )),
    }
}

/// Registers a session-wide shortcut via `org.freedesktop.portal.GlobalShortcuts`,
/// supported by GNOME 43+/KDE 6+ Wayland sessions. Compositors without this
/// portal (Sway, Hyprland without the impl, i3) simply never fire — users on
/// those should bind `ocr-translate capture` directly in their WM config instead.
pub(crate) fn spawn_portal_listener(tx: Sender<DaemonEvent>, description: String) {
    std::thread::spawn(move || {
        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => {
                tracing::warn!("could not start async runtime for GlobalShortcuts portal: {e}");
                return;
            }
        };
        if let Err(e) = rt.block_on(portal_shortcut_loop(tx, description)) {
            tracing::warn!(
                "GlobalShortcuts portal unavailable ({e}). This portal generally only \
                 grants global shortcuts to Flatpak/Snap-sandboxed apps; a plain binary \
                 gets 'An app id is required' even from a matching systemd scope. Use the \
                 tray's Capture menu item, or bind `ocr-translate capture` to a key \
                 yourself (on KDE: System Settings -> Shortcuts -> add a custom command \
                 shortcut; on Sway/Hyprland/i3: bind it in your compositor config)"
            );
        }
    });
}

async fn portal_shortcut_loop(tx: Sender<DaemonEvent>, description: String) -> Result<()> {
    use ashpd::desktop::global_shortcuts::{GlobalShortcuts, NewShortcut};
    use ashpd::WindowIdentifier;
    use futures_util::StreamExt;

    let shortcuts = GlobalShortcuts::new().await?;
    let session = shortcuts.create_session().await?;

    let shortcut = NewShortcut::new("capture_region", "Capture screen region for OCR translate")
        .preferred_trigger(Some(description.as_str()));
    shortcuts
        .bind_shortcuts(&session, &[shortcut], &WindowIdentifier::default())
        .await?
        .response()?;

    let mut activated = shortcuts.receive_activated().await?;
    while let Some(signal) = activated.next().await {
        if signal.shortcut_id() == "capture_region" {
            let _ = tx.send(DaemonEvent::Capture);
        }
    }
    Ok(())
}
