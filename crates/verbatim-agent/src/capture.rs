//! The desktop capture the end-to-end recordings take
//! (`verbatim-agent capture-desktop`): DXGI Desktop Duplication of the
//! first output, written to an ffmpeg child as raw frames at a steady
//! frame rate.
//!
//! ffmpeg's own desktop source, `gdigrab`, takes each frame with a GDI blit
//! of the screen, which waits for the compositor to render a GDI copy of
//! the desktop. On a machine whose compositor renders in software, a
//! console host drawing a flood held that blit for 1.7 to 3.3 seconds per
//! 200 lines (measured 2026-10-24 with no Verbatim running), so recordings
//! of the console host's floods stopped short of their scenario's end or
//! could not be stopped in time. Duplication takes the frames the
//! compositor presents and never waits on a GDI copy: through floods of 200
//! and 2,000 lines on that machine, no gap between frames passed 81
//! milliseconds. ffmpeg's duplication sources (`ddagrab`, `gfxcapture`)
//! need a Direct3D 11 device with video support, which the Microsoft Basic
//! Render Driver does not offer, so the agent duplicates the output itself
//! on a plain device and hands ffmpeg raw frames.
//!
//! A frame is written every frame interval, on the agent's high-resolution
//! pacing timer ([`crate::pace::Pacer`]), the newest the compositor
//! presented, or the last one again when nothing changed, so a still
//! screen is still recorded to the end. ffmpeg stamps each frame with the
//! wall-clock time it reads it (`-use_wallclock_as_timestamps`), which it
//! logs for the first as the input's `start:`, as `gdigrab` did. The
//! capture stops as ffmpeg does, on `q` (or the end) of its standard
//! input: it closes ffmpeg's input, ffmpeg encodes every frame it was
//! given and exits, and the capture exits with ffmpeg's code. The mouse
//! pointer is not drawn.

use std::io::{self, Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::pace::Pacer;

use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_FLAG, D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE,
    D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING, D3D11CreateDevice, ID3D11Device,
    ID3D11DeviceContext, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::{
    DXGI_ERROR_WAIT_TIMEOUT, DXGI_OUTDUPL_FRAME_INFO, IDXGIDevice, IDXGIOutput1,
    IDXGIOutputDuplication, IDXGIResource,
};
use windows::core::Interface;

/// Captures the desktop at `framerate` frames a second into ffmpeg at
/// `ffmpeg`, with `output_args` naming what ffmpeg makes of the frames,
/// until `q` or the end of standard input. Returns ffmpeg's exit code.
///
/// # Errors
///
/// Returns an error if the output cannot be duplicated, or duplicated again
/// once lost, a frame cannot be copied or is not the size the capture
/// began at, ffmpeg cannot be started, or writing to it fails before it was
/// told to stop: a frame is never left stale in silence.
pub fn run(framerate: u32, ffmpeg: &str, output_args: &[String]) -> io::Result<i32> {
    let mut duplication = Duplication::new()?;
    let (width, height) = duplication.size;
    let mut child = start_ffmpeg(ffmpeg, (width, height, framerate), output_args)?;
    let mut input = child
        .stdin
        .take()
        .ok_or_else(|| io::Error::other("ffmpeg has no standard input"))?;
    let stop = Arc::new(AtomicBool::new(false));
    watch_stdin(Arc::clone(&stop));
    let interval_ms = 1000 / framerate.clamp(1, 1000);
    let mut pacer = Pacer::new(interval_ms)?;
    let started = Instant::now();
    let interval = Duration::from_millis(u64::from(interval_ms));
    let mut frame = vec![0u8; width * height * 4];
    let mut slot = 0usize;
    while !stop.load(Ordering::Relaxed) {
        pacer.wait_for(slot)?;
        // The newest frame the compositor presented since the last slot.
        if let Err(error) = duplication.take_newest(&mut frame) {
            let _ = child.kill();
            return Err(error);
        }
        if let Err(error) = input.write_all(&frame) {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            let _ = child.kill();
            return Err(io::Error::other(format!(
                "writing a frame to ffmpeg failed: {error}"
            )));
        }
        // The next slot after now: a capture that fell behind goes on from
        // the present rather than writing the slots it missed in a burst.
        let elapsed = started.elapsed().as_millis() / interval.as_millis().max(1);
        slot = (slot + 1).max(usize::try_from(elapsed).unwrap_or(usize::MAX) + 1);
    }
    drop(input);
    exit_code(&mut child)
}

/// Starts ffmpeg reading raw frames of `width` by `height` at `framerate`
/// from its standard input, each stamped with the wall-clock time it reads
/// it, its log on the capture's standard error.
fn start_ffmpeg(
    ffmpeg: &str,
    (width, height, framerate): (usize, usize, u32),
    output_args: &[String],
) -> io::Result<Child> {
    let mut args: Vec<String> = [
        "-hide_banner",
        "-nostats",
        "-y",
        "-use_wallclock_as_timestamps",
        "1",
        "-f",
        "rawvideo",
        "-pix_fmt",
        "bgra",
        "-video_size",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    args.push(format!("{width}x{height}"));
    args.extend(["-framerate".to_owned(), framerate.to_string()]);
    args.extend(["-i".to_owned(), "pipe:0".to_owned()]);
    args.extend(output_args.iter().cloned());
    Command::new(ffmpeg)
        .args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
}

/// Sets `stop` once `q` arrives on standard input, or it ends.
fn watch_stdin(stop: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        let mut byte = [0u8; 1];
        let mut stdin = io::stdin();
        loop {
            match stdin.read(&mut byte) {
                Ok(0) | Err(_) => break,
                Ok(_) if byte[0] == b'q' => break,
                Ok(_) => {}
            }
        }
        stop.store(true, Ordering::Relaxed);
    });
}

/// Waits for ffmpeg to exit, and gives its code, -1 when it had none.
fn exit_code(child: &mut Child) -> io::Result<i32> {
    Ok(child.wait()?.code().unwrap_or(-1))
}

/// The first output's duplication, with a texture its frames are copied
/// into for reading.
struct Duplication {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    output: IDXGIOutputDuplication,
    staging: ID3D11Texture2D,
    size: (usize, usize),
}

impl Duplication {
    /// Duplicates the first output of the default adapter.
    fn new() -> io::Result<Self> {
        let mut device = None;
        let mut context = None;
        // SAFETY: the out pointers point at locals that outlive the call;
        // no adapter, software module or feature levels are passed.
        unsafe {
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_FLAG(0),
                None,
                D3D11_SDK_VERSION,
                Some(&raw mut device),
                None,
                Some(&raw mut context),
            )
        }
        .map_err(|error| io::Error::other(format!("no Direct3D 11 device: {error}")))?;
        let device: ID3D11Device = device.ok_or_else(|| io::Error::other("no device"))?;
        let context = context.ok_or_else(|| io::Error::other("no device context"))?;
        let duplication = duplicate(&device)?;
        // SAFETY: the duplication is a live interface.
        let desc = unsafe { duplication.GetDesc() };
        let (width, height) = (desc.ModeDesc.Width, desc.ModeDesc.Height);
        let texture = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_STAGING,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0.cast_unsigned(),
            ..D3D11_TEXTURE2D_DESC::default()
        };
        let mut staging = None;
        // SAFETY: the description is complete and the out pointer points at
        // a local; no initial data is given.
        unsafe { device.CreateTexture2D(&raw const texture, None, Some(&raw mut staging)) }
            .map_err(io::Error::other)?;
        let staging = staging.ok_or_else(|| io::Error::other("no staging texture"))?;
        let size = (
            usize::try_from(width).map_err(io::Error::other)?,
            usize::try_from(height).map_err(io::Error::other)?,
        );
        Ok(Self {
            device,
            context,
            output: duplication,
            staging,
            size,
        })
    }

    /// Copies into `frame` the newest frame the compositor presented since
    /// the last call, without waiting, leaving it as it was when none was.
    /// A duplication lost, as on a mode change, is made again at once; it
    /// fails if that cannot be done, a frame cannot be copied, or either is
    /// another size ([`same_size`]).
    fn take_newest(&mut self, frame: &mut [u8]) -> io::Result<()> {
        loop {
            let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
            let mut resource: Option<IDXGIResource> = None;
            // SAFETY: the out pointers point at locals that outlive the call.
            let acquired = unsafe {
                self.output
                    .AcquireNextFrame(0, &raw mut info, &raw mut resource)
            };
            match acquired {
                Ok(()) => {
                    let copied = match resource {
                        Some(resource) if info.LastPresentTime != 0 => self.copy(&resource, frame),
                        _ => Ok(()),
                    };
                    // SAFETY: a frame was acquired, and is released once.
                    let _ = unsafe { self.output.ReleaseFrame() };
                    copied?;
                }
                Err(error) if error.code() == DXGI_ERROR_WAIT_TIMEOUT => return Ok(()),
                Err(_) => {
                    let duplication = duplicate(&self.device)?;
                    // SAFETY: the duplication is a live interface.
                    let mode = unsafe { duplication.GetDesc() }.ModeDesc;
                    same_size(self.size, mode.Width, mode.Height)?;
                    self.output = duplication;
                }
            }
        }
    }

    /// Copies the acquired frame `resource` into `frame`, row by row.
    fn copy(&self, resource: &IDXGIResource, frame: &mut [u8]) -> io::Result<()> {
        let texture: ID3D11Texture2D = resource.cast().map_err(io::Error::other)?;
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        // SAFETY: the texture is live and the out pointer points at a local.
        unsafe { texture.GetDesc(&raw mut desc) };
        same_size(self.size, desc.Width, desc.Height)?;
        // SAFETY: both textures are live, and alike in size and format.
        unsafe { self.context.CopyResource(&self.staging, &texture) };
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        // SAFETY: the staging texture is CPU-readable and the out pointer
        // points at a local.
        unsafe {
            self.context
                .Map(&self.staging, 0, D3D11_MAP_READ, 0, Some(&raw mut mapped))
        }
        .map_err(|error| io::Error::other(format!("a frame could not be read: {error}")))?;
        let (width, height) = self.size;
        let row = width * 4;
        let pitch = mapped.RowPitch as usize;
        for (y, out) in frame.chunks_exact_mut(row).take(height).enumerate() {
            // SAFETY: the mapping holds `height` rows `pitch` bytes apart,
            // so row `y` starts inside it.
            let start = unsafe { mapped.pData.cast::<u8>().add(y * pitch) };
            // SAFETY: each row is at least `row` bytes long, and the mapping
            // lasts until it is unmapped below.
            let source = unsafe { std::slice::from_raw_parts(start, row) };
            out.copy_from_slice(source);
        }
        // SAFETY: the texture was mapped above.
        unsafe { self.context.Unmap(&self.staging, 0) };
        Ok(())
    }
}

/// Fails unless a frame or duplication of `width` by `height` is the
/// capture's `size`, which its staging texture and ffmpeg's input have.
fn same_size(size: (usize, usize), width: u32, height: u32) -> io::Result<()> {
    if (width as usize, height as usize) == size {
        return Ok(());
    }
    Err(io::Error::other(format!(
        "the desktop became {width}x{height} where the capture began at {}x{}",
        size.0, size.1
    )))
}

/// Duplicates the first output of `device`'s adapter.
fn duplicate(device: &ID3D11Device) -> io::Result<IDXGIOutputDuplication> {
    let dxgi: IDXGIDevice = device.cast().map_err(io::Error::other)?;
    // SAFETY: the DXGI device is live.
    let adapter = unsafe { dxgi.GetAdapter() }.map_err(io::Error::other)?;
    // SAFETY: the adapter is live; output 0 is its first.
    let output = unsafe { adapter.EnumOutputs(0) }.map_err(io::Error::other)?;
    let output: IDXGIOutput1 = output.cast().map_err(io::Error::other)?;
    // SAFETY: the output and the device are live, and the device is on the
    // output's adapter.
    unsafe { output.DuplicateOutput(device) }
        .map_err(|error| io::Error::other(format!("the desktop cannot be duplicated: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A frame or duplication of another size than the capture began at
    /// fails it, naming both sizes.
    #[test]
    fn a_duplication_of_another_size_fails_the_capture() {
        assert!(same_size((1920, 1080), 1920, 1080).is_ok());
        let error = same_size((1920, 1080), 1280, 720).expect_err("another size");
        assert_eq!(
            error.to_string(),
            "the desktop became 1280x720 where the capture began at 1920x1080"
        );
    }
}
