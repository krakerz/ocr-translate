use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc::sync_channel;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use pipewire as pw;
use pw::spa;
use pw::spa::param::format::{MediaSubtype, MediaType};
use pw::spa::param::format_utils;
use pw::spa::param::video::{VideoFormat, VideoInfoRaw};
use pw::spa::pod::Pod;

/// One decoded frame from the ScreenCast stream, kept as raw pixel bytes
/// (not yet converted to RGBA) — conversion is deferred to [`RawFrame::to_rgba`],
/// called only when a frame is actually requested (`ScreenCastSession::latest_frame`),
/// since the PipeWire `process` callback fires on every compositor frame
/// (commonly 60fps) but this app only samples occasionally.
struct RawFrame {
    format: VideoFormat,
    width: u32,
    height: u32,
    stride: usize,
    bytes: Vec<u8>,
}

impl RawFrame {
    fn to_rgba(&self) -> Option<image::RgbaImage> {
        let row_stride = if self.stride > 0 {
            self.stride
        } else {
            self.width as usize * 4
        };
        let mut out = image::RgbaImage::new(self.width, self.height);
        for y in 0..self.height as usize {
            let row_start = y * row_stride;
            let row_end = row_start + self.width as usize * 4;
            if row_end > self.bytes.len() {
                break;
            }
            let row = &self.bytes[row_start..row_end];
            for x in 0..self.width as usize {
                let px = &row[x * 4..x * 4 + 4];
                let rgba = match self.format {
                    VideoFormat::BGRx => [px[2], px[1], px[0], 255],
                    VideoFormat::BGRA => [px[2], px[1], px[0], px[3]],
                    VideoFormat::RGBx => [px[0], px[1], px[2], 255],
                    VideoFormat::RGBA => [px[0], px[1], px[2], px[3]],
                    _ => return None,
                };
                out.put_pixel(x as u32, y as u32, image::Rgba(rgba));
            }
        }
        Some(out)
    }
}

/// A long-lived `org.freedesktop.portal.ScreenCast` + PipeWire session:
/// negotiates the portal once — this is where the compositor shows its own
/// source picker, a single time — then keeps a background thread running a
/// PipeWire main loop that stores each new frame into a shared "latest
/// frame" slot. Call [`latest_frame`](Self::latest_frame) to poll it; there
/// is no per-frame callback or queue, since a live OCR loop only needs
/// occasional samples; the mailbox naturally coalesces to the newest frame.
pub struct ScreenCastSession {
    latest_frame: Arc<Mutex<Option<RawFrame>>>,
    _thread: std::thread::JoinHandle<()>,
}

impl ScreenCastSession {
    /// Blocks until the portal negotiation finishes (including however long
    /// the user takes to respond to the compositor's source picker), then
    /// returns with the PipeWire stream already running in the background.
    pub fn start() -> Result<Self> {
        let latest_frame = Arc::new(Mutex::new(None));
        let thread_frame = latest_frame.clone();
        let (ready_tx, ready_rx) = sync_channel::<Result<()>>(1);

        let thread = std::thread::spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    let _ =
                        ready_tx
                            .send(Err(e).context(
                                "failed to start async runtime for the ScreenCast portal",
                            ));
                    return;
                }
            };
            rt.block_on(async {
                if let Err(e) = negotiate_and_run(thread_frame, &ready_tx).await {
                    let _ = ready_tx.send(Err(e));
                }
            });
        });

        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Self {
                latest_frame,
                _thread: thread,
            }),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(anyhow::anyhow!(
                "ScreenCast session thread exited before it was ready"
            )),
        }
    }

    /// The most recently received frame, decoded to RGBA on demand. `None`
    /// if the stream hasn't delivered a frame yet.
    pub fn latest_frame(&self) -> Option<image::RgbaImage> {
        self.latest_frame
            .lock()
            .unwrap()
            .as_ref()
            .and_then(RawFrame::to_rgba)
    }
}

async fn negotiate_and_run(
    latest_frame: Arc<Mutex<Option<RawFrame>>>,
    ready_tx: &std::sync::mpsc::SyncSender<Result<()>>,
) -> Result<()> {
    use ashpd::desktop::screencast::{CursorMode, Screencast, SourceType};
    use ashpd::desktop::PersistMode;
    use ashpd::WindowIdentifier;

    let proxy = Screencast::new()
        .await
        .context("failed to connect to the ScreenCast portal")?;
    let session = proxy
        .create_session()
        .await
        .context("failed to create a ScreenCast session")?;
    proxy
        .select_sources(
            &session,
            CursorMode::Hidden,
            SourceType::Monitor.into(),
            false,
            None,
            PersistMode::DoNot,
        )
        .await
        .context("failed to configure the ScreenCast session")?;
    let streams = proxy
        .start(&session, &WindowIdentifier::default())
        .await
        .context("ScreenCast start request failed")?
        .response()
        .context("ScreenCast session was denied or cancelled")?;
    let stream = streams
        .streams()
        .first()
        .context("portal returned no streams to capture")?;
    let node_id = stream.pipe_wire_node_id();
    let fd = proxy
        .open_pipe_wire_remote(&session)
        .await
        .context("failed to open the PipeWire remote")?;

    // Negotiation succeeded; the caller can return from `start()` now. The
    // PipeWire loop below runs on a blocking-pool thread while this async
    // function (and the `proxy`/`session` it owns) stays alive by awaiting
    // it — dropping the portal session would end the stream.
    let _ = ready_tx.send(Ok(()));

    tokio::task::spawn_blocking(move || run_pipewire_loop(fd, node_id, latest_frame))
        .await
        .context("PipeWire capture thread panicked")??;
    Ok(())
}

fn run_pipewire_loop(
    fd: std::os::fd::OwnedFd,
    node_id: u32,
    latest_frame: Arc<Mutex<Option<RawFrame>>>,
) -> Result<()> {
    pw::init();

    let mainloop =
        pw::main_loop::MainLoopRc::new(None).context("failed to create PipeWire main loop")?;
    let context = pw::context::ContextRc::new(&mainloop, None)
        .context("failed to create PipeWire context")?;
    let core = context
        .connect_fd_rc(fd, None)
        .context("failed to connect to the PipeWire remote")?;

    let props = pw::properties::properties! {
        *pw::keys::MEDIA_TYPE => "Video",
        *pw::keys::MEDIA_CATEGORY => "Capture",
        *pw::keys::MEDIA_ROLE => "Screen",
    };
    let stream = pw::stream::StreamRc::new(core, "ocr-translate-screencast", props)
        .context("failed to create PipeWire stream")?;

    let negotiated_format = Rc::new(RefCell::new(VideoInfoRaw::new()));
    let format_for_cb = negotiated_format.clone();

    let _listener = stream
        .add_local_listener::<()>()
        .param_changed(move |_, _, id, param| {
            let Some(param) = param else { return };
            if id != spa::param::ParamType::Format.as_raw() {
                return;
            }
            let Ok((media_type, media_subtype)) = format_utils::parse_format(param) else {
                return;
            };
            if media_type != MediaType::Video || media_subtype != MediaSubtype::Raw {
                return;
            }
            if let Err(e) = format_for_cb.borrow_mut().parse(param) {
                tracing::warn!("failed to parse negotiated screencast video format: {e}");
                return;
            }
            let info = format_for_cb.borrow();
            let size = info.size();
            tracing::debug!(
                "negotiated screencast format {:?} {}x{}",
                info.format(),
                size.width,
                size.height
            );
        })
        .process(move |stream, _| {
            let Some(mut buffer) = stream.dequeue_buffer() else {
                return;
            };
            let datas = buffer.datas_mut();
            let Some(data) = datas.first_mut() else {
                return;
            };
            let chunk_size = data.chunk().size() as usize;
            let stride = data.chunk().stride().max(0) as usize;
            let Some(bytes) = data.data() else { return };
            let len = chunk_size.min(bytes.len());
            if len == 0 {
                return;
            }

            let info = negotiated_format.borrow();
            let size = info.size();
            if size.width == 0 || size.height == 0 {
                return;
            }

            *latest_frame.lock().unwrap() = Some(RawFrame {
                format: info.format(),
                width: size.width,
                height: size.height,
                stride,
                bytes: bytes[..len].to_vec(),
            });
        })
        .register()
        .context("failed to register PipeWire stream listener")?;

    // Offer several common raw formats as alternatives; the compositor
    // picks whichever it actually supports and `param_changed` above tells
    // us which one won.
    let mut format_bytes = Vec::new();
    for format in [
        VideoFormat::BGRx,
        VideoFormat::RGBx,
        VideoFormat::BGRA,
        VideoFormat::RGBA,
    ] {
        let obj = spa::pod::Object {
            type_: spa::utils::SpaTypes::ObjectParamFormat.as_raw(),
            id: spa::param::ParamType::EnumFormat.as_raw(),
            properties: video_format_properties(format),
        };
        let bytes = spa::pod::serialize::PodSerializer::serialize(
            std::io::Cursor::new(Vec::new()),
            &spa::pod::Value::Object(obj),
        )
        .context("failed to serialize a candidate video format")?
        .0
        .into_inner();
        format_bytes.push(bytes);
    }
    let mut params: Vec<&Pod> = format_bytes
        .iter()
        .map(|b| Pod::from_bytes(b).expect("just-serialized format Pod must be valid"))
        .collect();

    stream
        .connect(
            spa::utils::Direction::Input,
            Some(node_id),
            pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS,
            &mut params,
        )
        .context("failed to connect the PipeWire stream to the ScreenCast source")?;

    mainloop.run();
    Ok(())
}

/// Builds the `SPA_TYPE_OBJECT_Format` properties for "raw video, this
/// pixel format, any size/framerate" — `libspa`'s `VideoInfoRaw` has no
/// `Into<Vec<Property>>` impl (unlike `AudioInfoRaw`, which the pipewire-rs
/// audio-capture example relies on for the equivalent step), so this
/// replicates that conversion by hand for the one field we care about.
fn video_format_properties(format: VideoFormat) -> Vec<spa::pod::Property> {
    use spa::pod::{Property, Value};
    use spa::sys as spa_sys;
    use spa::utils::Id;

    vec![
        Property::new(
            spa_sys::SPA_FORMAT_mediaType,
            Value::Id(Id(spa_sys::SPA_MEDIA_TYPE_video)),
        ),
        Property::new(
            spa_sys::SPA_FORMAT_mediaSubtype,
            Value::Id(Id(spa_sys::SPA_MEDIA_SUBTYPE_raw)),
        ),
        Property::new(
            spa_sys::SPA_FORMAT_VIDEO_format,
            Value::Id(Id(format.as_raw())),
        ),
    ]
}
