//! WASAPI event-driven shared-mode [`AudioSink`] (decision D5).
//!
//! One render stream at a time, driven by the synth thread that owns the
//! sink: [`begin`](AudioSink::begin) opens (or reuses) a shared-mode client
//! for the utterance's [`PcmFormat`], [`write`](AudioSink::write) blocks on
//! the render event while feeding 16-bit PCM into the device buffer,
//! [`end`](AudioSink::end) lets buffered audio drain, and
//! [`stop`](AudioSink::stop) discards it immediately to interrupt speech.
//!
//! The client is initialized with `AUTOCONVERTPCM` and `SRC_DEFAULT_QUALITY`
//! so the device accepts any synth sample rate, and with a small (~40 ms)
//! buffer in service of the keypress-to-audio latency budget (architecture
//! section 6). Sample-rate conversion and format matching are the driver's
//! concern only in that the reported [`PcmFormat`] must be truthful.

use tracing::trace;
use windows::Win32::Foundation::{CloseHandle, HANDLE, RPC_E_CHANGED_MODE, WAIT_OBJECT_0};
use windows::Win32::Media::Audio::{
    AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
    AUDCLNT_STREAMFLAGS_EVENTCALLBACK, AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, IAudioClient,
    IAudioRenderClient, IMMDeviceEnumerator, MMDeviceEnumerator, WAVE_FORMAT_PCM, WAVEFORMATEX,
    eConsole, eRender,
};
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
};
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
use windows::core::{HRESULT, PCWSTR};

use crate::{AudioError, AudioSink, PcmFormat};

/// Requested render buffer, in 100-nanosecond units: about 40 ms.
const BUFFER_DURATION_HNS: i64 = 400_000;

/// Bits per sample of the PCM the whole M1 pipeline carries.
const BITS_PER_SAMPLE: u16 = 16;

/// How long a single `write` waits on the render event before giving up and
/// reporting the stream as failed (the event fires once per ~40 ms buffer
/// period, so this is a wide safety margin, not a tuning knob).
const RENDER_WAIT_MS: u32 = 2_000;

/// Maps a `windows` COM error onto an [`AudioError`], tagging it as a device
/// or stream failure per the calling context.
fn device_error(context: &str, error: &windows::core::Error) -> AudioError {
    AudioError::Device(format!("{context}: {error}"))
}

fn stream_error(context: &str, error: &windows::core::Error) -> AudioError {
    AudioError::Stream(format!("{context}: {error}"))
}

/// An initialized render stream for one [`PcmFormat`], reused across
/// utterances of the same format.
struct Stream {
    client: IAudioClient,
    render: IAudioRenderClient,
    event: HANDLE,
    format: PcmFormat,
    buffer_frames: u32,
}

impl Drop for Stream {
    fn drop(&mut self) {
        // Best-effort teardown; the process is usually exiting anyway.
        unsafe {
            let _ = self.client.Stop();
            if !self.event.is_invalid() {
                let _ = CloseHandle(self.event);
            }
        }
    }
}

/// WASAPI implementation of [`AudioSink`].
///
/// Created cheaply with [`WasapiSink::new`]; the audio device is opened lazily
/// on the first [`begin`](AudioSink::begin) so constructing a sink never
/// fails and a missing device surfaces only when audio is actually needed.
pub struct WasapiSink {
    enumerator: Option<IMMDeviceEnumerator>,
    stream: Option<Stream>,
    /// The current utterance's trace id, tagged onto the audio-started event.
    trace_id: Option<verbatim_model::TraceId>,
    /// Whether the current utterance has submitted its first buffer yet.
    first_buffer_submitted: bool,
    /// Whether the current stream is running (between `begin` and
    /// `end`/`stop`).
    running: bool,
}

// The sink owns its COM objects and is moved to, then used only from, the one
// synth thread that drives it; it is never shared between threads. The raw
// event `HANDLE` is what makes the struct non-`Send` by default.
unsafe impl Send for WasapiSink {}

impl Default for WasapiSink {
    fn default() -> Self {
        Self::new()
    }
}

impl WasapiSink {
    /// Creates a sink that opens the default render device on first use.
    #[must_use]
    pub fn new() -> Self {
        Self {
            enumerator: None,
            stream: None,
            trace_id: None,
            first_buffer_submitted: false,
            running: false,
        }
    }

    /// Initializes COM for this thread as MTA, tolerating a prior STA init in
    /// the same thread (`RPC_E_CHANGED_MODE`): WASAPI works from either
    /// apartment and we never uninitialize.
    fn ensure_com() -> Result<(), AudioError> {
        // Safe: no reserved parameter, standard apartment selection.
        let hr: HRESULT = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if hr.is_err() && hr != RPC_E_CHANGED_MODE {
            return Err(AudioError::Device(format!(
                "CoInitializeEx failed: {:#010x}",
                hr.0
            )));
        }
        Ok(())
    }

    /// Returns the cached device enumerator, creating it on first use.
    fn enumerator(&mut self) -> Result<IMMDeviceEnumerator, AudioError> {
        if let Some(enumerator) = &self.enumerator {
            return Ok(enumerator.clone());
        }
        // Safe: standard class activation of the system device enumerator.
        let enumerator: IMMDeviceEnumerator =
            unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
                .map_err(|error| device_error("create device enumerator", &error))?;
        self.enumerator = Some(enumerator.clone());
        Ok(enumerator)
    }

    /// Builds and initializes a fresh render stream for `format`.
    fn open_stream(&mut self, format: PcmFormat) -> Result<Stream, AudioError> {
        let enumerator = self.enumerator()?;
        let block_align = format.channels * (BITS_PER_SAMPLE / 8);
        let wave_format = WAVEFORMATEX {
            wFormatTag: u16::try_from(WAVE_FORMAT_PCM).expect("WAVE_FORMAT_PCM fits in u16"),
            nChannels: format.channels,
            nSamplesPerSec: format.sample_rate,
            nAvgBytesPerSec: format.sample_rate * u32::from(block_align),
            nBlockAlign: block_align,
            wBitsPerSample: BITS_PER_SAMPLE,
            cbSize: 0,
        };

        // Safe: every call checks its result; pointers outlive the calls.
        unsafe {
            let device = enumerator
                .GetDefaultAudioEndpoint(eRender, eConsole)
                .map_err(|error| device_error("no default render device", &error))?;
            let client: IAudioClient = device
                .Activate(CLSCTX_ALL, None)
                .map_err(|error| device_error("activate audio client", &error))?;
            let flags = AUDCLNT_STREAMFLAGS_EVENTCALLBACK
                | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
                | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;
            client
                .Initialize(
                    AUDCLNT_SHAREMODE_SHARED,
                    flags,
                    BUFFER_DURATION_HNS,
                    0,
                    &raw const wave_format,
                    None,
                )
                .map_err(|error| device_error("initialize audio client", &error))?;

            let event = CreateEventW(None, false, false, PCWSTR::null())
                .map_err(|error| device_error("create render event", &error))?;
            client
                .SetEventHandle(event)
                .map_err(|error| device_error("set render event", &error))?;
            let buffer_frames = client
                .GetBufferSize()
                .map_err(|error| device_error("query buffer size", &error))?;
            let render: IAudioRenderClient = client
                .GetService()
                .map_err(|error| device_error("get render client", &error))?;

            Ok(Stream {
                client,
                render,
                event,
                format,
                buffer_frames,
            })
        }
    }

    /// Feeds one contiguous run of frames into the device, waiting on the
    /// render event whenever the buffer is full.
    fn feed(
        stream: &Stream,
        samples: &[i16],
        first: &mut bool,
        trace_id: verbatim_model::TraceId,
    ) -> Result<(), AudioError> {
        let channels = usize::from(stream.format.channels);
        debug_assert!(channels > 0);
        let mut offset = 0usize;
        let total_frames = samples.len() / channels;
        while offset < total_frames {
            // Wait for the device to signal it can take another buffer.
            // Safe: the event handle lives as long as the stream.
            let wait = unsafe { WaitForSingleObject(stream.event, RENDER_WAIT_MS) };
            if wait != WAIT_OBJECT_0 {
                return Err(AudioError::Stream(format!(
                    "render event wait returned {:#010x}",
                    wait.0
                )));
            }
            // Safe: padding is always in the range 0..=buffer_frames.
            let padding = unsafe { stream.client.GetCurrentPadding() }
                .map_err(|error| stream_error("query padding", &error))?;
            let free_frames = stream.buffer_frames.saturating_sub(padding);
            if free_frames == 0 {
                continue;
            }
            let remaining = u32::try_from(total_frames - offset).unwrap_or(u32::MAX);
            let frames = free_frames.min(remaining);
            let frames_usize = frames as usize;
            // Safe: we request no more than the reported free frame count and
            // release exactly what we wrote.
            let data = unsafe { stream.render.GetBuffer(frames) }
                .map_err(|error| stream_error("acquire render buffer", &error))?;
            let sample_count = frames_usize * channels;
            let src = &samples[offset * channels..offset * channels + sample_count];
            unsafe {
                std::ptr::copy_nonoverlapping(
                    src.as_ptr().cast::<u8>(),
                    data,
                    sample_count * std::mem::size_of::<i16>(),
                );
                stream
                    .render
                    .ReleaseBuffer(frames, 0)
                    .map_err(|error| stream_error("release render buffer", &error))?;
            }
            if !*first {
                *first = true;
                // The final leg of the keypress-to-audio timeline: the first
                // buffer of this utterance has reached the device.
                trace!(target: "verbatim::audio", trace_id = %trace_id, "audio_started");
            }
            offset += frames_usize;
        }
        Ok(())
    }
}

impl AudioSink for WasapiSink {
    fn begin(
        &mut self,
        format: PcmFormat,
        trace_id: verbatim_model::TraceId,
    ) -> Result<(), AudioError> {
        Self::ensure_com()?;

        // Reuse the stream when the format is unchanged; otherwise rebuild.
        let reuse = self
            .stream
            .as_ref()
            .is_some_and(|stream| stream.format == format);
        if !reuse {
            self.stream = None;
            let stream = self.open_stream(format)?;
            self.stream = Some(stream);
        }

        let stream = self.stream.as_ref().expect("stream present after open");
        // A reused stream was left stopped by the previous end/stop; reset its
        // padding to zero and start it fresh for this utterance.
        // Safe: the client is initialized and owned by this sink.
        unsafe {
            let _ = stream.client.Reset();
            stream
                .client
                .Start()
                .map_err(|error| device_error("start audio client", &error))?;
        }
        self.trace_id = Some(trace_id);
        self.first_buffer_submitted = false;
        self.running = true;
        Ok(())
    }

    fn write(&mut self, samples: &[i16]) -> Result<(), AudioError> {
        if samples.is_empty() {
            return Ok(());
        }
        let stream = self
            .stream
            .as_ref()
            .ok_or_else(|| AudioError::Stream("write before begin".to_owned()))?;
        let trace_id = self
            .trace_id
            .ok_or_else(|| AudioError::Stream("write before begin".to_owned()))?;
        let mut first = self.first_buffer_submitted;
        let result = Self::feed(stream, samples, &mut first, trace_id);
        self.first_buffer_submitted = first;
        result
    }

    fn end(&mut self) -> Result<(), AudioError> {
        let Some(stream) = self.stream.as_ref() else {
            return Ok(());
        };
        if self.running {
            // Let queued audio drain: wait until the device has played out
            // everything before stopping, bounded by the render-wait margin.
            loop {
                // Safe: initialized client owned by this sink.
                let padding = unsafe { stream.client.GetCurrentPadding() }
                    .map_err(|error| stream_error("query padding on drain", &error))?;
                if padding == 0 {
                    break;
                }
                let wait = unsafe { WaitForSingleObject(stream.event, RENDER_WAIT_MS) };
                if wait != WAIT_OBJECT_0 {
                    break;
                }
            }
            // Safe: stopping an initialized, started client.
            unsafe {
                let _ = stream.client.Stop();
            }
            self.running = false;
        }
        Ok(())
    }

    fn stop(&mut self) {
        if let Some(stream) = self.stream.as_ref() {
            // Discard immediately: stop the clock, then reset to drop any
            // audio still buffered in the device.
            // Safe: initialized client owned by this sink.
            unsafe {
                let _ = stream.client.Stop();
                let _ = stream.client.Reset();
            }
        }
        self.running = false;
        self.first_buffer_submitted = false;
    }
}
