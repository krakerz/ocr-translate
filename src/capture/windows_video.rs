use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use image::RgbaImage;

use super::windows_backend;

/// Windows equivalent of `screencast::ScreenCastSession` (Linux) for Live
/// Region Translate: a continuous screen-capture session, backed by `xcap`'s
/// `Monitor::video_recorder()` — DXGI Desktop Duplication under the hood,
/// since the `wgc` feature is disabled (see the Cargo.toml comment on the
/// `xcap` dependency), same as one-shot capture's `grab_active_monitor`.
/// Unlike the portal-based Linux session, this needs no user "share screen
/// with..." picker dialog at all.
///
/// `xcap` delivers frames on an `mpsc::Receiver`, one at a time — this spawns
/// a thread that drains it into a shared "latest frame" mailbox (same
/// coalesce-to-newest pattern as `screencast::RawFrame`, for the same
/// reason: a poll-driven OCR loop only ever wants the newest frame, not a
/// queue of everything that arrived since the last poll).
///
/// Confirmed delivering real frames on a Windows 11 VM (the known upstream
/// `xcap` issue about `video_recorder()` failing under a VM did not
/// reproduce), but never run on physical hardware, and region-selection
/// pixel alignment hasn't been stress-tested. See NOTES.md.
pub struct RegionSession {
    recorder: xcap::VideoRecorder,
    latest_frame: Arc<Mutex<Option<RgbaImage>>>,
    _thread: std::thread::JoinHandle<()>,
}

impl RegionSession {
    pub fn start() -> Result<Self> {
        let monitor = windows_backend::active_monitor()?;
        let (recorder, rx) = monitor
            .video_recorder()
            .context("failed to start xcap video recorder")?;

        let latest_frame = Arc::new(Mutex::new(None));
        let thread_frame = latest_frame.clone();
        let thread = std::thread::spawn(move || {
            while let Ok(frame) = rx.recv() {
                if let Some(img) = RgbaImage::from_raw(frame.width, frame.height, frame.raw) {
                    *thread_frame.lock().unwrap() = Some(img);
                }
            }
        });

        recorder
            .start()
            .context("failed to start xcap screen recording")?;

        Ok(Self {
            recorder,
            latest_frame,
            _thread: thread,
        })
    }

    /// The most recently received frame. `None` if the recorder hasn't
    /// delivered a frame yet.
    pub fn latest_frame(&self) -> Option<RgbaImage> {
        self.latest_frame.lock().unwrap().clone()
    }
}

impl Drop for RegionSession {
    fn drop(&mut self) {
        let _ = self.recorder.stop();
    }
}
