//! The WASAPI output device (decisions D5 and D17).
//!
//! An event-driven shared-mode stream on the system's default render device,
//! in the device's own mix rate and channel layout as 32-bit float, so the
//! mixer's output needs no further conversion. The buffer is small (about
//! 40 ms) so a discarded queue is a short one; the mixer measures underruns
//! to check that it is not too small.
//!
//! Recovery. The device registers once, on the audio thread, for Windows'
//! endpoint notifications, and passes them to [`DeviceNotices`]: an open
//! stream is reopened when the default render device changes, and any
//! failure of the stream (a headset unplugged reports
//! `AUDCLNT_E_DEVICE_INVALIDATED`) is an error the mixer answers by
//! reopening. When an open fails, the mixer plays real-time silence in the
//! device's place, and the device asks to be opened again on the evidence
//! for its failure ([`open_failure`]): when there is no render device, at
//! the next notification that a device arrived, became active, or became
//! the default; for any other failure, which Windows need send no such
//! notification for clearing, also a second later.

use std::sync::Arc;
use std::time::Duration;

use tracing::{info, warn};
use verbatim_audio::{AudioDevice, AudioError, ClockReading, DeviceFormat, DeviceNotices, Waker};
use windows::Win32::Foundation::{CloseHandle, HANDLE, RPC_E_CHANGED_MODE};
use windows::Win32::Media::Audio::{
    AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
    AUDCLNT_STREAMFLAGS_EVENTCALLBACK, AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, IAudioClient,
    IAudioClock, IAudioRenderClient, IMMDeviceEnumerator, IMMNotificationClient,
    MMDeviceEnumerator, WAVEFORMATEX, WAVEFORMATEXTENSIBLE, WAVEFORMATEXTENSIBLE_0, eConsole,
    eRender,
};
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
};
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows::Win32::System::Threading::{
    AvSetMmThreadCharacteristicsW, CreateEventW, INFINITE, SetEvent, WaitForMultipleObjects,
    WaitForSingleObject,
};
use windows::core::{GUID, HRESULT, PCWSTR, w};

use watcher::DeviceWatcher;

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

/// How long after a failed open, other than for a missing device, to try
/// it again, since Windows sends no notification when such a failure
/// clears ([`open_failure`]).
const OPEN_RETRY: Duration = Duration::from_secs(1);

fn device_error(context: &str, error: &windows::core::Error) -> AudioError {
    AudioError::Device(format!("{context}: {error}"))
}

/// An auto-reset event handle, closed on drop. Waiting and signalling are
/// thread-safe, which is what lets the waker run on any thread.
struct Event(HANDLE);

// SAFETY: the handle is a kernel event object, which may be waited on,
// signalled, and closed from any thread.
unsafe impl Send for Event {}
// SAFETY: as above; waiting and signalling are thread-safe operations.
unsafe impl Sync for Event {}

impl Event {
    fn new() -> Result<Self, AudioError> {
        // SAFETY: an unnamed auto-reset event with default security.
        unsafe { CreateEventW(None, false, false, PCWSTR::null()) }
            .map(Self)
            .map_err(|error| device_error("create event", &error))
    }
}

impl Drop for Event {
    fn drop(&mut self) {
        // SAFETY: the handle is owned and closed once.
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

    use verbatim_audio::{DeviceNotice, DeviceNotices};
    use windows::Win32::Foundation::PROPERTYKEY;
    use windows::Win32::Media::Audio::{
        DEVICE_STATE, DEVICE_STATE_ACTIVE, EDataFlow, ERole, IMMNotificationClient,
        IMMNotificationClient_Impl, eConsole, eRender,
    };
    use windows::core::PCWSTR;
    use windows_core::implement;

    /// Passes Windows' endpoint notifications to the device's notices,
    /// which ask for a reopen and wake the audio thread when one could
    /// help. An arrival or activation is not filtered by data flow, since
    /// telling a capture device from a render one would take a call into
    /// the enumerator from the callback; it costs a reopen only while an
    /// open has failed.
    #[implement(IMMNotificationClient)]
    pub(super) struct DeviceWatcher {
        pub(super) notices: DeviceNotices,
    }

    impl IMMNotificationClient_Impl for DeviceWatcher_Impl {
        fn OnDeviceStateChanged(
            &self,
            _device: &PCWSTR,
            state: DEVICE_STATE,
        ) -> windows::core::Result<()> {
            if state == DEVICE_STATE_ACTIVE {
                self.notices.notify(DeviceNotice::Available);
            }
            Ok(())
        }

        fn OnDeviceAdded(&self, _device: &PCWSTR) -> windows::core::Result<()> {
            self.notices.notify(DeviceNotice::Available);
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
                self.notices.notify(DeviceNotice::DefaultChanged);
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
    /// The stream's clock, with its frequency in units a second and the
    /// stream's sample rate, for the latency ledger's device clock stage.
    clock: IAudioClock,
    clock_frequency: u64,
    sample_rate: u32,
    event: Event,
    channels: usize,
    running: bool,
}

/// What a failed open waits for before it is tried again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Evidence {
    /// A notification: a device arrived, became active, or became the
    /// default.
    Notification,
    /// A deadline as well as a notification, since Windows sends none when
    /// the failure clears.
    Deadline,
}

/// What evidence an open that failed with `error` waits for.
///
/// - No render device (`GetDefaultAudioEndpoint` returns `E_NOTFOUND`):
///   Windows reports the next one as it arrives (`OnDeviceAdded`), becomes
///   active (`OnDeviceStateChanged`) and becomes the default
///   (`OnDefaultDeviceChanged`).
/// - Anything else is tried again at a deadline, [`OPEN_RETRY`], as well,
///   since the endpoint notifications report endpoints arriving, leaving,
///   changing state or becoming the default, and an endpoint's properties
///   changing, never that a failure on a device still there has cleared.
///   An active device refuses the stream when another program holds it in
///   exclusive mode (`AUDCLNT_E_DEVICE_IN_USE`), the audio service is not
///   running, or the engine lacked a resource. Checked on 2026-10-21: with
///   the default device held in exclusive mode, a shared open failed with
///   `AUDCLNT_E_DEVICE_IN_USE`, releasing the device raised no
///   notification in the five seconds after, and a shared open then
///   succeeded.
/// - `AUDCLNT_E_DEVICE_INVALIDATED` is among them: besides a device that
///   went away, it reports a device reconfigured in place, which stays the
///   default and active. Checked on 2026-10-21 by changing the default
///   device's format: every open stream failed with it, and an open made
///   while the engine was still reconfiguring failed with it too. Windows
///   raised only property changes, all before that open returned (in one
///   trial before it began, so even passing them on would not have asked
///   for another open), and nothing in the five seconds after; an open
///   about 60 ms later succeeded.
fn open_failure(error: &windows::core::Error) -> Evidence {
    if error.code() == E_NOTFOUND {
        Evidence::Notification
    } else {
        Evidence::Deadline
    }
}

/// A stream that could not be opened: the error, and what the next open
/// waits for.
struct OpenFailure {
    error: AudioError,
    evidence: Evidence,
}

impl From<AudioError> for OpenFailure {
    fn from(error: AudioError) -> Self {
        Self {
            error,
            evidence: Evidence::Deadline,
        }
    }
}

/// `context` and the Windows `error`, as a failed open, with the evidence
/// its next open waits for.
fn open_error(context: &str, error: &windows::core::Error) -> OpenFailure {
    OpenFailure {
        error: device_error(context, error),
        evidence: open_failure(error),
    }
}

/// The WASAPI implementation of [`AudioDevice`].
///
/// Constructing it opens nothing; the mixer's audio thread calls
/// [`AudioDevice::open`], which initializes COM on that thread.
pub struct WasapiDevice {
    enumerator: Option<IMMDeviceEnumerator>,
    /// The notification client, once registered with `enumerator`.
    watcher: Option<IMMNotificationClient>,
    stream: Option<Stream>,
    notices: DeviceNotices,
    wake: Arc<Event>,
}

// SAFETY: the MMDevice and audio client objects are free-threaded objects
// created in the multithreaded apartment, so moving them to another thread
// is allowed, provided they are used and released on a thread that has
// joined COM; the mixer opens, uses, and drops the device on its audio
// thread, which has. The struct is not `Sync`, so it is never shared.
unsafe impl Send for WasapiDevice {}

impl WasapiDevice {
    /// A device that opens the default render endpoint when the mixer asks.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::Device`] when the wake event cannot be created.
    pub fn new() -> Result<Self, AudioError> {
        let wake = Arc::new(Event::new()?);
        Ok(Self {
            enumerator: None,
            watcher: None,
            stream: None,
            notices: DeviceNotices::new(waker(&wake)),
            wake,
        })
    }

    /// The device enumerator, created on first use with the watcher for
    /// endpoint notifications registered on it.
    fn enumerator(&mut self) -> Result<IMMDeviceEnumerator, AudioError> {
        if let Some(enumerator) = &self.enumerator {
            return Ok(enumerator.clone());
        }
        // SAFETY: no reserved parameter. WASAPI works from either apartment,
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
        // SAFETY: a constant task name and a valid out-pointer.
        let registered =
            unsafe { AvSetMmThreadCharacteristicsW(w!("Pro Audio"), &raw mut task_index) };
        if let Err(error) = registered {
            warn!(target: "verbatim::audio", %error, "the audio thread could not be registered with MMCSS");
        }
        // SAFETY: standard activation of the system device enumerator.
        let enumerator: IMMDeviceEnumerator =
            unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
                .map_err(|error| device_error("create device enumerator", &error))?;
        let watcher: IMMNotificationClient = DeviceWatcher {
            notices: self.notices.clone(),
        }
        .into();
        // SAFETY: the watcher is kept alive in `self` for as long as the
        // enumerator it is registered with.
        match unsafe { enumerator.RegisterEndpointNotificationCallback(&watcher) } {
            Ok(()) => self.watcher = Some(watcher),
            Err(error) => {
                warn!(target: "verbatim::audio", %error, "device changes will not be followed, and a failed open is tried again each second");
            }
        }
        self.enumerator = Some(enumerator.clone());
        Ok(enumerator)
    }

    /// Opens a shared-mode stream on the current default render device.
    fn open_stream(&mut self) -> Result<(Stream, DeviceFormat), OpenFailure> {
        let enumerator = self.enumerator()?;
        // The COM calls below are plain calls on objects this thread owns.
        // SAFETY: as above.
        let device = unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eConsole) }
            .map_err(|error| open_error("no default render device", &error))?;
        // SAFETY: as above.
        let client: IAudioClient = unsafe { device.Activate(CLSCTX_ALL, None) }
            .map_err(|error| open_error("activate audio client", &error))?;
        // SAFETY: as above; the mix format pointer is read once and freed
        // with CoTaskMemFree below.
        let mix = unsafe { client.GetMixFormat() }
            .map_err(|error| open_error("read the mix format", &error))?;
        // SAFETY: `mix` points to the format the call allocated, at least a
        // WAVEFORMATEX, which is packed, so any alignment will do.
        let header = unsafe { mix.read_unaligned() };
        let sample_rate = header.nSamplesPerSec;
        let channels = header.nChannels;
        let channel_mask = if header.wFormatTag == WAVE_FORMAT_EXTENSIBLE {
            // SAFETY: the tag says the allocation is a WAVEFORMATEXTENSIBLE,
            // also packed.
            unsafe { mix.cast::<WAVEFORMATEXTENSIBLE>().read_unaligned() }.dwChannelMask
        } else {
            0
        };
        // SAFETY: the format was allocated for this caller with the COM task
        // allocator and is not used again.
        unsafe { CoTaskMemFree(Some(mix.cast_const().cast())) };

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
        // SAFETY: as for the calls above; `format` outlives the call, which
        // copies it.
        unsafe {
            client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_EVENTCALLBACK
                    | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
                    | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
                BUFFER_DURATION_HNS,
                0,
                (&raw const format).cast(),
                None,
            )
        }
        .map_err(|error| open_error("initialize the stream", &error))?;
        let event = Event::new()?;
        // SAFETY: as above; the event outlives the client, as `Stream`'s
        // field order ensures.
        unsafe { client.SetEventHandle(event.0) }
            .map_err(|error| open_error("set the render event", &error))?;
        // SAFETY: as above.
        let buffer_frames = unsafe { client.GetBufferSize() }
            .map_err(|error| open_error("read the buffer size", &error))?;
        // SAFETY: as above.
        let render: IAudioRenderClient = unsafe { client.GetService() }
            .map_err(|error| open_error("get the render client", &error))?;
        // SAFETY: as above.
        let clock: IAudioClock = unsafe { client.GetService() }
            .map_err(|error| open_error("get the stream's clock", &error))?;
        // SAFETY: a plain COM call on the clock just obtained.
        let clock_frequency = unsafe { clock.GetFrequency() }
            .map_err(|error| open_error("read the clock's frequency", &error))?;
        Ok((
            Stream {
                client,
                render,
                clock,
                clock_frequency,
                sample_rate,
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

impl Drop for WasapiDevice {
    fn drop(&mut self) {
        if let (Some(enumerator), Some(watcher)) = (&self.enumerator, &self.watcher) {
            // SAFETY: unregistering the callback registered in `enumerator`.
            unsafe {
                let _ = enumerator.UnregisterEndpointNotificationCallback(watcher);
            }
        }
    }
}

/// A waker that signals `wake`.
fn waker(wake: &Arc<Event>) -> Waker {
    let wake = Arc::clone(wake);
    Arc::new(move || {
        // SAFETY: signalling an event this closure keeps alive.
        unsafe {
            let _ = SetEvent(wake.0);
        }
    })
}

impl WasapiDevice {
    /// The open stream, or the error for a device that is not open.
    fn stream(&mut self) -> Result<&mut Stream, AudioError> {
        self.stream
            .as_mut()
            .ok_or_else(|| AudioError::Device("the device is not open".to_owned()))
    }
}

impl AudioDevice for WasapiDevice {
    fn open(&mut self) -> Result<DeviceFormat, AudioError> {
        self.stop();
        self.stream = None;
        self.notices.opening();
        match self.open_stream() {
            Ok((stream, format)) => {
                info!(
                    target: "verbatim::audio",
                    sample_rate = format.sample_rate,
                    channels = format.channels,
                    buffer_frames = format.buffer_frames,
                    "audio device opened"
                );
                self.notices.opened();
                self.stream = Some(stream);
                Ok(format)
            }
            Err(OpenFailure { error, evidence }) => {
                // Without a registered watcher no notification comes, so
                // every failure waits for the deadline.
                if evidence == Evidence::Notification && self.watcher.is_some() {
                    info!(target: "verbatim::audio", "the audio device will be opened again when Windows reports a device that could open");
                    self.notices.failed(None);
                } else {
                    info!(target: "verbatim::audio", "the audio device will be opened again in a second, since Windows reports nothing when this failure clears");
                    self.notices.failed(Some(OPEN_RETRY));
                }
                Err(error)
            }
        }
    }

    fn queued_frames(&mut self) -> Result<u32, AudioError> {
        let stream = self.stream()?;
        // SAFETY: a plain COM call on the stream this device owns.
        unsafe { stream.client.GetCurrentPadding() }
            .map_err(|error| device_error("read the queue length", &error))
    }

    fn write(&mut self, samples: &[f32]) -> Result<(), AudioError> {
        let stream = self.stream()?;
        let frames = samples.len() / stream.channels;
        if frames == 0 {
            // Less than a frame is nothing to play, and GetBuffer(0) need
            // not return a pointer to copy into.
            return Ok(());
        }
        let frame_count = u32::try_from(frames)
            .map_err(|_| AudioError::Stream("write larger than the buffer".to_owned()))?;
        let bytes = frames * stream.channels * std::mem::size_of::<f32>();
        // SAFETY: GetBuffer returns room for `frames` frames of the stream's
        // float format, `bytes` bytes, which is exactly what is copied, byte
        // by byte, so the buffer's alignment, which WASAPI does not
        // document, does not matter; `frames` is not zero, so the pointer is
        // valid. The buffer is released with the same frame count.
        let buffer = unsafe { stream.render.GetBuffer(frame_count) }
            .map_err(|error| device_error("get the render buffer", &error))?;
        // SAFETY: as above: `buffer` has room for `bytes` bytes, and
        // `samples` holds at least that many.
        unsafe {
            std::ptr::copy_nonoverlapping(samples.as_ptr().cast::<u8>(), buffer, bytes);
        }
        // SAFETY: as above.
        unsafe { stream.render.ReleaseBuffer(frame_count, 0) }
            .map_err(|error| device_error("release the render buffer", &error))
    }

    fn start(&mut self) -> Result<(), AudioError> {
        let stream = self.stream()?;
        if !stream.running {
            // SAFETY: a plain COM call on the stream this device owns.
            unsafe { stream.client.Start() }
                .map_err(|error| device_error("start the stream", &error))?;
            stream.running = true;
        }
        Ok(())
    }

    fn stop(&mut self) {
        if let Some(stream) = &mut self.stream {
            // A stream that fails these is already broken, and the mixer
            // reopens it on the next error it sees.
            // SAFETY: a plain COM call on the stream this device owns.
            let _ = unsafe { stream.client.Stop() };
            // SAFETY: as above.
            let _ = unsafe { stream.client.Reset() };
            stream.running = false;
        }
    }

    fn clock_position(&mut self) -> Option<ClockReading> {
        let stream = self.stream.as_ref()?;
        let mut position = 0u64;
        // The performance counter's value when the position was taken, in
        // 100-nanosecond units.
        let mut taken = 0u64;
        // SAFETY: a plain COM call on the stream's clock, writing two locals.
        unsafe {
            stream
                .clock
                .GetPosition(&raw mut position, Some(&raw mut taken))
        }
        .ok()?;
        let mut counter = 0i64;
        let mut frequency = 0i64;
        // SAFETY: each writes one local; neither fails on Windows XP or later.
        unsafe { QueryPerformanceCounter(&raw mut counter) }.ok()?;
        // SAFETY: as above.
        unsafe { QueryPerformanceFrequency(&raw mut frequency) }.ok()?;
        let now =
            u128::try_from(counter).ok()? * 10_000_000 / u128::try_from(frequency).ok()?.max(1);
        let age_units = now.saturating_sub(u128::from(taken));
        let frames = u128::from(position) * u128::from(stream.sample_rate)
            / u128::from(stream.clock_frequency.max(1));
        Some(ClockReading {
            frames: u64::try_from(frames).ok()?,
            age: Duration::from_nanos(u64::try_from(age_units * 100).ok()?),
        })
    }

    fn wait(&mut self, timeout: Duration) {
        // A notification that asks for a reopen signals `wake` from its
        // callback; a failure retried at a deadline ends the wait by then.
        let timeout = self.notices.wait_limit(timeout);
        let timeout_ms = u32::try_from(timeout.as_millis()).unwrap_or(INFINITE);
        // Each wait is on event handles this device owns.
        match &self.stream {
            Some(stream) if stream.running => {
                // SAFETY: as above.
                let _ = unsafe {
                    WaitForMultipleObjects(&[stream.event.0, self.wake.0], false, timeout_ms)
                };
            }
            Some(_) | None => {
                // SAFETY: as above.
                let _ = unsafe { WaitForSingleObject(self.wake.0, timeout_ms) };
            }
        }
    }

    fn waker(&self) -> Waker {
        waker(&self.wake)
    }

    fn needs_reopen(&self) -> bool {
        self.notices.needs_reopen()
    }
}

#[cfg(test)]
mod tests {
    use windows::Win32::Media::Audio::AUDCLNT_E_DEVICE_INVALIDATED;

    use super::*;

    #[test]
    fn an_open_that_finds_the_device_invalidated_is_tried_again_at_the_deadline() {
        let error = windows::core::Error::from_hresult(AUDCLNT_E_DEVICE_INVALIDATED);
        assert_eq!(open_failure(&error), Evidence::Deadline);
    }
}
