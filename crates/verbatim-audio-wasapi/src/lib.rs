//! The WASAPI output device (decisions D5 and D17).
//!
//! An event-driven shared-mode stream on the system's default render device,
//! in the device's own mix rate and channel layout as 32-bit float, so the
//! mixer's output needs no further conversion. The buffer is small (about
//! 40 ms) so a discarded queue is a short one; the mixer measures underruns
//! to check that it is not too small.
//!
//! Recovery. The device asks to be reopened when Windows reports that the
//! default render device changed, and any failure of the stream (a headset
//! unplugged reports `AUDCLNT_E_DEVICE_INVALIDATED`) is an error the mixer
//! answers by reopening. When no render device exists at all, the device
//! plays at real-time speed into silence, as [`verbatim_audio::SilentDevice`]
//! does, until Windows reports a new default device.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use tracing::{info, warn};
use verbatim_audio::{AudioDevice, AudioError, DeviceFormat, SilentDevice, Waker};
use windows::Win32::Foundation::{CloseHandle, HANDLE, RPC_E_CHANGED_MODE};
use windows::Win32::Media::Audio::{
    AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
    AUDCLNT_STREAMFLAGS_EVENTCALLBACK, AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, IAudioClient,
    IAudioRenderClient, IMMDeviceEnumerator, IMMNotificationClient, MMDeviceEnumerator,
    WAVEFORMATEX, WAVEFORMATEXTENSIBLE, WAVEFORMATEXTENSIBLE_0, eConsole, eRender,
};
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
};
use windows::Win32::System::Threading::{
    AvSetMmThreadCharacteristicsW, CreateEventW, INFINITE, SetEvent, WaitForMultipleObjects,
    WaitForSingleObject,
};
use windows::core::{GUID, HRESULT, PCWSTR, w};

use watcher::DefaultDeviceWatcher;

/// Requested render buffer, in 100-nanosecond units: about 40 ms.
const BUFFER_DURATION_HNS: i64 = 400_000;

/// `WAVE_FORMAT_EXTENSIBLE`, the format tag of a [`WAVEFORMATEXTENSIBLE`].
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;

/// `KSDATAFORMAT_SUBTYPE_IEEE_FLOAT`: 32-bit float samples.
const SUBTYPE_IEEE_FLOAT: GUID = GUID::from_u128(0x0000_0003_0000_0010_8000_00aa_0038_9b71);

/// `E_NOTFOUND`, what `GetDefaultAudioEndpoint` returns when the system has
/// no render device: `HRESULT_FROM_WIN32(ERROR_NOT_FOUND)`.
#[expect(
    clippy::cast_possible_wrap,
    reason = "an HRESULT is the bit pattern of a u32"
)]
const E_NOTFOUND: HRESULT = HRESULT(0x8007_0490_u32 as i32);

/// How long to play silently after a device failed to open before trying
/// it again.
const OPEN_RETRY: Duration = Duration::from_secs(1);

/// How often the silent fallback wakes while playing: 10 ms, a typical
/// device period.
const SILENT_PERIOD_MS: u32 = 10;

fn device_error(context: &str, error: &windows::core::Error) -> AudioError {
    AudioError::Device(format!("{context}: {error}"))
}

/// An auto-reset event handle, closed on drop. Waiting and signalling are
/// thread-safe, which is what lets the waker run on any thread.
struct Event(HANDLE);

// The handle is a kernel object, usable from any thread.
unsafe impl Send for Event {}
unsafe impl Sync for Event {}

impl Event {
    fn new() -> Result<Self, AudioError> {
        // Safe: an unnamed auto-reset event with default security.
        unsafe { CreateEventW(None, false, false, PCWSTR::null()) }
            .map(Self)
            .map_err(|error| device_error("create event", &error))
    }
}

impl Drop for Event {
    fn drop(&mut self) {
        // Safe: the handle is owned and closed once.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

/// The `#[implement]`-generated COM object lives in its own module so the
/// module-level allow covers the macro's generated glue (which uses
/// `#[inline(always)]` and reference-to-raw-pointer casts the pedantic group
/// flags) without loosening the lint for hand-written code.
mod watcher {
    #![allow(clippy::inline_always, clippy::ref_as_ptr)]

    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use windows::Win32::Foundation::PROPERTYKEY;
    use windows::Win32::Media::Audio::{
        DEVICE_STATE, EDataFlow, ERole, IMMNotificationClient, IMMNotificationClient_Impl,
        eConsole, eRender,
    };
    use windows::Win32::System::Threading::SetEvent;
    use windows::core::PCWSTR;
    use windows_core::implement;

    use super::Event;

    /// Sets the reopen flag, and wakes the audio thread, when the default
    /// render device changes.
    #[implement(IMMNotificationClient)]
    pub(super) struct DefaultDeviceWatcher {
        pub(super) changed: Arc<AtomicBool>,
        pub(super) wake: Arc<Event>,
    }

    impl IMMNotificationClient_Impl for DefaultDeviceWatcher_Impl {
        fn OnDeviceStateChanged(
            &self,
            _device: &PCWSTR,
            _state: DEVICE_STATE,
        ) -> windows::core::Result<()> {
            Ok(())
        }

        fn OnDeviceAdded(&self, _device: &PCWSTR) -> windows::core::Result<()> {
            Ok(())
        }

        fn OnDeviceRemoved(&self, _device: &PCWSTR) -> windows::core::Result<()> {
            Ok(())
        }

        fn OnDefaultDeviceChanged(
            &self,
            flow: EDataFlow,
            role: ERole,
            _device: &PCWSTR,
        ) -> windows::core::Result<()> {
            if flow == eRender && role == eConsole {
                self.changed.store(true, Ordering::SeqCst);
                // Safe: the event outlives the watcher, which holds it.
                unsafe {
                    let _ = SetEvent(self.wake.0);
                }
            }
            Ok(())
        }

        fn OnPropertyValueChanged(
            &self,
            _device: &PCWSTR,
            _key: &PROPERTYKEY,
        ) -> windows::core::Result<()> {
            Ok(())
        }
    }
}

/// An open shared-mode stream.
struct Stream {
    client: IAudioClient,
    render: IAudioRenderClient,
    event: Event,
    channels: usize,
    running: bool,
}

/// Why a stream could not be opened.
enum OpenFailure {
    /// The system has no render device at all.
    NoDevice(AudioError),
    /// A render device exists but could not be opened; this may pass.
    Failed(AudioError),
}

impl From<AudioError> for OpenFailure {
    fn from(error: AudioError) -> Self {
        Self::Failed(error)
    }
}

/// What the device is playing through.
enum Output {
    Closed,
    Stream(Stream),
    /// No render device exists; real time into silence.
    Silent(SilentDevice),
}

/// The WASAPI implementation of [`AudioDevice`].
///
/// Constructing it opens nothing; the mixer's audio thread calls
/// [`AudioDevice::open`], which initializes COM on that thread.
pub struct WasapiDevice {
    enumerator: Option<IMMDeviceEnumerator>,
    watcher: Option<IMMNotificationClient>,
    output: Output,
    default_changed: Arc<AtomicBool>,
    /// When to try opening a real device again after one failed to open.
    retry_at: Option<Instant>,
    wake: Arc<Event>,
}

// The COM objects are created and used only on the mixer's audio thread,
// which owns the device; the struct is moved there before first use.
unsafe impl Send for WasapiDevice {}

impl WasapiDevice {
    /// A device that opens the default render endpoint when the mixer asks.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::Device`] when the wake event cannot be created.
    pub fn new() -> Result<Self, AudioError> {
        Ok(Self {
            enumerator: None,
            watcher: None,
            output: Output::Closed,
            default_changed: Arc::new(AtomicBool::new(false)),
            retry_at: None,
            wake: Arc::new(Event::new()?),
        })
    }

    /// The device enumerator, created on first use with a watcher for
    /// default-device changes registered on it.
    fn enumerator(&mut self) -> Result<IMMDeviceEnumerator, AudioError> {
        if let Some(enumerator) = &self.enumerator {
            return Ok(enumerator.clone());
        }
        // Safe: no reserved parameter. WASAPI works from either apartment,
        // and COM is never uninitialized on this thread.
        let hr: HRESULT = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if hr.is_err() && hr != RPC_E_CHANGED_MODE {
            return Err(AudioError::Device(format!(
                "CoInitializeEx failed: {:#010x}",
                hr.0
            )));
        }
        // This runs once, on the audio thread that drives the device: tell
        // the Multimedia Class Scheduler Service it renders audio, so it is
        // scheduled ahead of ordinary work and keeps the small buffer fed
        // while Verbatim or the system is busy (decision D15). The thread
        // keeps the registration for its life.
        let mut task_index = 0u32;
        // Safe: a constant task name and a valid out-pointer.
        if let Err(error) =
            unsafe { AvSetMmThreadCharacteristicsW(w!("Pro Audio"), &raw mut task_index) }
        {
            warn!(target: "verbatim::audio", %error, "the audio thread could not be registered with MMCSS");
        }
        // Safe: standard activation of the system device enumerator.
        let enumerator: IMMDeviceEnumerator =
            unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
                .map_err(|error| device_error("create device enumerator", &error))?;
        let watcher: IMMNotificationClient = DefaultDeviceWatcher {
            changed: Arc::clone(&self.default_changed),
            wake: Arc::clone(&self.wake),
        }
        .into();
        // Safe: the watcher is kept alive in `self` for as long as the
        // enumerator it is registered with.
        if let Err(error) = unsafe { enumerator.RegisterEndpointNotificationCallback(&watcher) } {
            warn!(target: "verbatim::audio", %error, "default device changes will not be followed");
        }
        self.watcher = Some(watcher);
        self.enumerator = Some(enumerator.clone());
        Ok(enumerator)
    }

    /// Opens a shared-mode stream on the current default render device.
    fn open_stream(&mut self) -> Result<(Stream, DeviceFormat), OpenFailure> {
        let enumerator = self.enumerator().map_err(OpenFailure::Failed)?;
        // Safe: plain COM calls on objects this thread owns; the mix format
        // pointer is read once and freed with CoTaskMemFree below.
        unsafe {
            let device = enumerator
                .GetDefaultAudioEndpoint(eRender, eConsole)
                .map_err(|error| {
                    let failure = device_error("no default render device", &error);
                    if error.code() == E_NOTFOUND {
                        OpenFailure::NoDevice(failure)
                    } else {
                        OpenFailure::Failed(failure)
                    }
                })?;
            let client: IAudioClient = device
                .Activate(CLSCTX_ALL, None)
                .map_err(|error| device_error("activate audio client", &error))?;
            let mix = client
                .GetMixFormat()
                .map_err(|error| device_error("read the mix format", &error))?;
            let sample_rate = (*mix).nSamplesPerSec;
            let channels = (*mix).nChannels;
            let channel_mask = if (*mix).wFormatTag == WAVE_FORMAT_EXTENSIBLE {
                (*mix.cast::<WAVEFORMATEXTENSIBLE>()).dwChannelMask
            } else {
                0
            };
            CoTaskMemFree(Some(mix.cast_const().cast()));

            let block_align = channels * 4;
            let format = WAVEFORMATEXTENSIBLE {
                Format: WAVEFORMATEX {
                    wFormatTag: WAVE_FORMAT_EXTENSIBLE,
                    nChannels: channels,
                    nSamplesPerSec: sample_rate,
                    nAvgBytesPerSec: sample_rate * u32::from(block_align),
                    nBlockAlign: block_align,
                    wBitsPerSample: 32,
                    cbSize: 22,
                },
                Samples: WAVEFORMATEXTENSIBLE_0 {
                    wValidBitsPerSample: 32,
                },
                dwChannelMask: channel_mask,
                SubFormat: SUBTYPE_IEEE_FLOAT,
            };
            client
                .Initialize(
                    AUDCLNT_SHAREMODE_SHARED,
                    AUDCLNT_STREAMFLAGS_EVENTCALLBACK
                        | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
                        | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
                    BUFFER_DURATION_HNS,
                    0,
                    (&raw const format).cast(),
                    None,
                )
                .map_err(|error| device_error("initialize the stream", &error))?;
            let event = Event::new()?;
            client
                .SetEventHandle(event.0)
                .map_err(|error| device_error("set the render event", &error))?;
            let buffer_frames = client
                .GetBufferSize()
                .map_err(|error| device_error("read the buffer size", &error))?;
            let render: IAudioRenderClient = client
                .GetService()
                .map_err(|error| device_error("get the render client", &error))?;
            Ok((
                Stream {
                    client,
                    render,
                    event,
                    channels: usize::from(channels),
                    running: false,
                },
                DeviceFormat {
                    sample_rate,
                    channels,
                    buffer_frames,
                },
            ))
        }
    }
}

impl Drop for WasapiDevice {
    fn drop(&mut self) {
        if let (Some(enumerator), Some(watcher)) = (&self.enumerator, &self.watcher) {
            // Safe: unregistering the callback registered in `enumerator`.
            unsafe {
                let _ = enumerator.UnregisterEndpointNotificationCallback(watcher);
            }
        }
    }
}

impl AudioDevice for WasapiDevice {
    fn open(&mut self) -> Result<DeviceFormat, AudioError> {
        self.stop();
        self.output = Output::Closed;
        self.default_changed.store(false, Ordering::SeqCst);
        self.retry_at = None;
        match self.open_stream() {
            Ok((stream, format)) => {
                info!(
                    target: "verbatim::audio",
                    sample_rate = format.sample_rate,
                    channels = format.channels,
                    buffer_frames = format.buffer_frames,
                    "audio device opened"
                );
                self.output = Output::Stream(stream);
                Ok(format)
            }
            Err(failure) => {
                // Speech must go on whatever the device does, so a device
                // that cannot be opened is replaced by real-time silence:
                // until Windows reports a new default device when there is
                // none, or for a second when the open failed.
                match failure {
                    OpenFailure::NoDevice(error) => {
                        warn!(target: "verbatim::audio", %error, "no audio device; playing silently in real time until one appears");
                    }
                    OpenFailure::Failed(error) => {
                        warn!(target: "verbatim::audio", %error, "the audio device could not be opened; playing silently and trying again in a second");
                        self.retry_at = Some(Instant::now() + OPEN_RETRY);
                    }
                }
                let mut silent = SilentDevice::new();
                let format = silent.open()?;
                self.output = Output::Silent(silent);
                Ok(format)
            }
        }
    }

    fn queued_frames(&mut self) -> Result<u32, AudioError> {
        match &mut self.output {
            Output::Closed => Err(AudioError::Device("the device is not open".to_owned())),
            // Safe: a plain COM call on the stream this device owns.
            Output::Stream(stream) => unsafe { stream.client.GetCurrentPadding() }
                .map_err(|error| device_error("read the queue length", &error)),
            Output::Silent(silent) => silent.queued_frames(),
        }
    }

    fn write(&mut self, samples: &[f32]) -> Result<(), AudioError> {
        match &mut self.output {
            Output::Closed => Err(AudioError::Device("the device is not open".to_owned())),
            Output::Stream(stream) => {
                let frames = samples.len() / stream.channels;
                let frame_count = u32::try_from(frames)
                    .map_err(|_| AudioError::Stream("write larger than the buffer".to_owned()))?;
                // Safe: GetBuffer returns room for `frames` frames of the
                // stream's float format, which is exactly what is copied; the
                // buffer is aligned for its format, so for f32.
                #[expect(
                    clippy::cast_ptr_alignment,
                    reason = "WASAPI buffers are aligned for their sample format"
                )]
                unsafe {
                    let buffer = stream
                        .render
                        .GetBuffer(frame_count)
                        .map_err(|error| device_error("get the render buffer", &error))?;
                    std::ptr::copy_nonoverlapping(
                        samples.as_ptr(),
                        buffer.cast::<f32>(),
                        frames * stream.channels,
                    );
                    stream
                        .render
                        .ReleaseBuffer(frame_count, 0)
                        .map_err(|error| device_error("release the render buffer", &error))
                }
            }
            Output::Silent(silent) => silent.write(samples),
        }
    }

    fn start(&mut self) -> Result<(), AudioError> {
        match &mut self.output {
            Output::Closed => Err(AudioError::Device("the device is not open".to_owned())),
            Output::Stream(stream) => {
                if !stream.running {
                    // Safe: a plain COM call on the stream this device owns.
                    unsafe { stream.client.Start() }
                        .map_err(|error| device_error("start the stream", &error))?;
                    stream.running = true;
                }
                Ok(())
            }
            Output::Silent(silent) => silent.start(),
        }
    }

    fn stop(&mut self) {
        match &mut self.output {
            Output::Closed => {}
            Output::Stream(stream) => {
                // Safe: plain COM calls on the stream this device owns. A
                // stream that fails these is already broken, and the mixer
                // reopens it on the next error it sees.
                unsafe {
                    let _ = stream.client.Stop();
                    let _ = stream.client.Reset();
                }
                stream.running = false;
            }
            Output::Silent(silent) => silent.stop(),
        }
    }

    fn wait(&mut self, timeout: Duration) {
        let timeout_ms = u32::try_from(timeout.as_millis()).unwrap_or(INFINITE - 1);
        // Safe: waiting on event handles this device owns.
        unsafe {
            match &self.output {
                Output::Stream(stream) if stream.running => {
                    let _ =
                        WaitForMultipleObjects(&[stream.event.0, self.wake.0], false, timeout_ms);
                }
                Output::Silent(silent) if silent.is_playing() => {
                    let _ = WaitForSingleObject(self.wake.0, timeout_ms.min(SILENT_PERIOD_MS));
                }
                Output::Closed | Output::Stream(_) | Output::Silent(_) => {
                    let _ = WaitForSingleObject(self.wake.0, timeout_ms);
                }
            }
        }
    }

    fn waker(&self) -> Waker {
        let wake = Arc::clone(&self.wake);
        Arc::new(move || {
            // Safe: signalling an event this closure keeps alive.
            unsafe {
                let _ = SetEvent(wake.0);
            }
        })
    }

    fn needs_reopen(&self) -> bool {
        self.default_changed.load(Ordering::SeqCst)
            || self.retry_at.is_some_and(|at| Instant::now() >= at)
    }
}
