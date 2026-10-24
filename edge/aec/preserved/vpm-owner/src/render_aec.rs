// SPDX-License-Identifier: LGPL-3.0-or-later
//
// Offline render-reference receiver and alignment candidate.
//
// This module deliberately stops before claiming an AEC capability. It owns
// the private FVRR1 transport, performs the wire-format conversion and
// 24 kHz -> 16 kHz resampling, aligns 20 ms pairs, and feeds the official
// SpeexDSP backend when the static candidate feature is enabled. Any malformed
// packet, sequence discontinuity, stale render stream, or timeline drift makes
// the candidate fail closed. No PCM is written to disk.

use std::collections::VecDeque;
use std::ffi::{c_char, c_int, c_void, CStr};
use std::fs::Permissions;
use std::io::{self, ErrorKind};
use std::os::unix::fs::{PermissionsExt, FileTypeExt};
use std::os::unix::net::UnixDatagram;
use std::time::{Duration, Instant};

const RTLD_NOW: c_int = 2;
const SPEEX_ECHO_SET_SAMPLING_RATE: c_int = 24;

#[cfg(any(test, feature = "speexdsp-dynamic-test"))]
unsafe extern "C" {
    fn dlopen(filename: *const c_char, flags: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    fn dlclose(handle: *mut c_void) -> c_int;
}

#[cfg(feature = "speexdsp-static")]
#[link(name = "speexdsp", kind = "static")]
unsafe extern "C" {
    fn speex_echo_state_init(frame_size: c_int, filter_length: c_int) -> *mut c_void;
    fn speex_echo_state_destroy(state: *mut c_void);
    fn speex_echo_state_reset(state: *mut c_void);
    fn speex_echo_cancellation(
        state: *mut c_void,
        microphone: *const i16,
        render: *const i16,
        residual: *mut i16,
    );
    fn speex_echo_ctl(state: *mut c_void, request: c_int, value: *mut c_void) -> c_int;
}

pub const RENDER_REFERENCE_SOCKET_PATH: &str =
    "/tmp/family-voice-vpm-render-reference.sock";
pub const FVRR1_MAGIC: &[u8] = b"FVRR1 PCM\n";
pub const FVRR1_START: &[u8] = b"FVRR1 START\n";
pub const FVRR1_STOP: &[u8] = b"FVRR1 STOP\n";
pub const RENDER_SAMPLE_RATE: u32 = 24_000;
pub const CAPTURE_SAMPLE_RATE: u32 = 16_000;
pub const FRAME_SAMPLES: usize = 320;
pub const MAX_RENDER_PCM_BYTES: usize = 16 * 1024;
pub const MAX_RENDER_PACKET_BYTES: usize = FVRR1_MAGIC.len() + 8 + 4 + MAX_RENDER_PCM_BYTES;
pub const DEFAULT_DELAY_MS: u32 = 80;
pub const DEFAULT_TIMEOUT_MS: u64 = 120;
pub const DEFAULT_DRIFT_TOLERANCE_MS: u32 = 20;
pub const DEFAULT_HISTORY_MS: u32 = 1_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderPacketError {
    InvalidMagic,
    InvalidLength,
    InvalidSequence,
    InvalidPcm,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RenderPacket<'a> {
    pub sequence: u64,
    pub pcm: &'a [u8],
}

pub fn parse_render_packet(packet: &[u8]) -> Result<RenderPacket<'_>, RenderPacketError> {
    if packet.len() < FVRR1_MAGIC.len() + 12 {
        return Err(RenderPacketError::InvalidLength);
    }
    if !packet.starts_with(FVRR1_MAGIC) {
        return Err(RenderPacketError::InvalidMagic);
    }
    let sequence_start = FVRR1_MAGIC.len();
    let sequence_end = sequence_start + 8;
    let size_end = sequence_end + 4;
    let sequence = u64::from_le_bytes(
        packet[sequence_start..sequence_end]
            .try_into()
            .map_err(|_| RenderPacketError::InvalidLength)?,
    );
    let size = u32::from_le_bytes(
        packet[sequence_end..size_end]
            .try_into()
            .map_err(|_| RenderPacketError::InvalidLength)?,
    ) as usize;
    if sequence == 0 {
        return Err(RenderPacketError::InvalidSequence);
    }
    if size == 0 || size > MAX_RENDER_PCM_BYTES || size % 2 != 0 {
        return Err(RenderPacketError::InvalidPcm);
    }
    if packet.len() != size_end + size {
        return Err(RenderPacketError::InvalidLength);
    }
    Ok(RenderPacket {
        sequence,
        pcm: &packet[size_end..],
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailClosedReason {
    NoRender,
    SequenceGap,
    StaleRender,
    Drift,
    RenderUnderflow,
    InvalidPacket,
    DspUnavailable,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AecMetrics {
    pub packets_received: u64,
    pub packets_rejected: u64,
    pub sequence_gaps: u64,
    pub stale_render_events: u64,
    pub drift_events: u64,
    pub render_underflow_events: u64,
    pub dsp_failures: u64,
    pub frames_aligned: u64,
    pub frames_rejected: u64,
    pub last_erle_db: Option<f32>,
    pub near_end_energy_ratio: Option<f32>,
}

#[derive(Clone, Copy, Debug)]
pub struct AlignmentConfig {
    pub delay_ms: u32,
    pub timeout_ms: u64,
    pub drift_tolerance_ms: u32,
    pub history_ms: u32,
}

impl Default for AlignmentConfig {
    fn default() -> Self {
        Self {
            delay_ms: DEFAULT_DELAY_MS,
            timeout_ms: DEFAULT_TIMEOUT_MS,
            drift_tolerance_ms: DEFAULT_DRIFT_TOLERANCE_MS,
            history_ms: DEFAULT_HISTORY_MS,
        }
    }
}

impl AlignmentConfig {
    fn delay_samples(self) -> u64 {
        u64::from(self.delay_ms) * u64::from(CAPTURE_SAMPLE_RATE) / 1_000
    }

    fn drift_tolerance_samples(self) -> i64 {
        i64::from(self.drift_tolerance_ms) * i64::from(CAPTURE_SAMPLE_RATE) / 1_000
    }

    fn history_samples(self) -> usize {
        (u64::from(self.history_ms) * u64::from(CAPTURE_SAMPLE_RATE) / 1_000)
            .try_into()
            .unwrap_or(FRAME_SAMPLES * 4)
    }
}

/// A frame pair is copied only for the downstream DSP call and is cleared when
/// dropped by its owner. The owner never serializes this value to disk.
#[derive(Debug, PartialEq)]
pub struct AlignedFrame {
    pub capture: Vec<i16>,
    pub render: Vec<i16>,
}

impl Drop for AlignedFrame {
    fn drop(&mut self) {
        self.capture.fill(0);
        self.render.fill(0);
    }
}

#[derive(Debug)]
struct Resampler24To16 {
    input: VecDeque<i16>,
    phase_half_samples: usize,
}

impl Resampler24To16 {
    fn new() -> Self {
        Self {
            input: VecDeque::new(),
            phase_half_samples: 0,
        }
    }

    fn reset(&mut self) {
        self.input.clear();
        self.phase_half_samples = 0;
    }

    fn push_pcm(&mut self, pcm: &[u8]) -> Vec<i16> {
        for sample in pcm.chunks_exact(2) {
            self.input.push_back(i16::from_le_bytes([sample[0], sample[1]]));
        }
        let mut output = Vec::with_capacity(self.input.len() * 2 / 3);
        while self.phase_half_samples / 2 + 1 < self.input.len() {
            let index = self.phase_half_samples / 2;
            let first = self.input[index] as i32;
            let value = if self.phase_half_samples % 2 == 0 {
                first
            } else {
                (first + self.input[index + 1] as i32) / 2
            };
            output.push(value.clamp(i16::MIN as i32, i16::MAX as i32) as i16);
            self.phase_half_samples += 3;
        }
        let removable = (self.phase_half_samples / 2).min(self.input.len().saturating_sub(1));
        for _ in 0..removable {
            let _ = self.input.pop_front();
        }
        self.phase_half_samples = self.phase_half_samples.saturating_sub(removable * 2);
        output
    }
}

impl Drop for Resampler24To16 {
    fn drop(&mut self) {
        for sample in &mut self.input {
            *sample = 0;
        }
        self.input.clear();
    }
}

type SpeexEchoStateInit = unsafe extern "C" fn(frame_size: c_int, filter_length: c_int) -> *mut c_void;
type SpeexEchoStateDestroy = unsafe extern "C" fn(state: *mut c_void);
type SpeexEchoStateReset = unsafe extern "C" fn(state: *mut c_void);
type SpeexEchoCancellation = unsafe extern "C" fn(
    state: *mut c_void,
    microphone: *const i16,
    render: *const i16,
    residual: *mut i16,
);
type SpeexEchoCtl = unsafe extern "C" fn(state: *mut c_void, request: c_int, value: *mut c_void) -> c_int;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpeexLoadError {
    LibraryUnavailable,
    SymbolUnavailable,
    StateUnavailable,
    SamplingRateRejected,
}

/// Thin FFI wrapper around the official SpeexDSP echo canceller. Target builds
/// statically link an archive verified by build.rs; the dynamic loader exists
/// only for the ignored host-side synthetic test. An absent or non-matching
/// target archive is a build failure instead of a silent fallback.
pub struct SpeexDspEcho {
    handle: *mut c_void,
    dynamic: bool,
    state: *mut c_void,
    destroy: SpeexEchoStateDestroy,
    reset: SpeexEchoStateReset,
    cancel: SpeexEchoCancellation,
}

impl SpeexDspEcho {
    /// Load a fixed-point SpeexDSP library supplied by the target image. The
    /// library itself must have been built from the official xiph/speexdsp
    /// source with `-DFIXED_POINT`; the loader does not pretend to verify a
    /// library's compilation flags.
    #[cfg(any(test, feature = "speexdsp-dynamic-test"))]
    pub unsafe fn load(path: &CStr, frame_size: usize, filter_length: usize) -> Result<Self, SpeexLoadError> {
        let handle = unsafe { dlopen(path.as_ptr(), RTLD_NOW) };
        if handle.is_null() {
            return Err(SpeexLoadError::LibraryUnavailable);
        }
        let init: SpeexEchoStateInit = match unsafe { symbol(handle, c"speex_echo_state_init") } {
            Some(value) => value,
            None => {
                unsafe { dlclose(handle) };
                return Err(SpeexLoadError::SymbolUnavailable);
            }
        };
        let destroy: SpeexEchoStateDestroy = match unsafe { symbol(handle, c"speex_echo_state_destroy") } {
            Some(value) => value,
            None => {
                unsafe { dlclose(handle) };
                return Err(SpeexLoadError::SymbolUnavailable);
            }
        };
        let reset: SpeexEchoStateReset = match unsafe { symbol(handle, c"speex_echo_state_reset") } {
            Some(value) => value,
            None => {
                unsafe { dlclose(handle) };
                return Err(SpeexLoadError::SymbolUnavailable);
            }
        };
        let cancel: SpeexEchoCancellation = match unsafe { symbol(handle, c"speex_echo_cancellation") } {
            Some(value) => value,
            None => {
                unsafe { dlclose(handle) };
                return Err(SpeexLoadError::SymbolUnavailable);
            }
        };
        let ctl: SpeexEchoCtl = match unsafe { symbol(handle, c"speex_echo_ctl") } {
            Some(value) => value,
            None => {
                unsafe { dlclose(handle) };
                return Err(SpeexLoadError::SymbolUnavailable);
            }
        };
        let frame_size = c_int::try_from(frame_size).map_err(|_| SpeexLoadError::StateUnavailable)?;
        let filter_length = c_int::try_from(filter_length).map_err(|_| SpeexLoadError::StateUnavailable)?;
        let state = unsafe { init(frame_size, filter_length) };
        if state.is_null() {
            unsafe { dlclose(handle) };
            return Err(SpeexLoadError::StateUnavailable);
        }
        let mut sample_rate = CAPTURE_SAMPLE_RATE as c_int;
        let ctl_result = unsafe {
            ctl(
                state,
                SPEEX_ECHO_SET_SAMPLING_RATE,
                (&mut sample_rate as *mut c_int).cast::<c_void>(),
            )
        };
        if ctl_result != 0 {
            unsafe {
                destroy(state);
                dlclose(handle);
            }
            return Err(SpeexLoadError::SamplingRateRejected);
        }
        Ok(Self {
            handle,
            dynamic: true,
            state,
            destroy,
            reset,
            cancel,
        })
    }

    #[cfg(feature = "speexdsp-static")]
    pub unsafe fn load_static(frame_size: usize, filter_length: usize) -> Result<Self, SpeexLoadError> {
        let frame_size = c_int::try_from(frame_size).map_err(|_| SpeexLoadError::StateUnavailable)?;
        let filter_length = c_int::try_from(filter_length).map_err(|_| SpeexLoadError::StateUnavailable)?;
        let state = unsafe { speex_echo_state_init(frame_size, filter_length) };
        if state.is_null() {
            return Err(SpeexLoadError::StateUnavailable);
        }
        let mut sample_rate = CAPTURE_SAMPLE_RATE as c_int;
        let ctl_result = unsafe {
            speex_echo_ctl(
                state,
                SPEEX_ECHO_SET_SAMPLING_RATE,
                (&mut sample_rate as *mut c_int).cast::<c_void>(),
            )
        };
        if ctl_result != 0 {
            unsafe { speex_echo_state_destroy(state) };
            return Err(SpeexLoadError::SamplingRateRejected);
        }
        Ok(Self {
            handle: std::ptr::null_mut(),
            dynamic: false,
            state,
            destroy: speex_echo_state_destroy,
            reset: speex_echo_state_reset,
            cancel: speex_echo_cancellation,
        })
    }

    pub fn process(&mut self, microphone: &[i16], render: &[i16]) -> Result<Vec<i16>, SpeexLoadError> {
        if microphone.len() != FRAME_SAMPLES || render.len() != FRAME_SAMPLES {
            return Err(SpeexLoadError::StateUnavailable);
        }
        let mut residual = vec![0i16; FRAME_SAMPLES];
        unsafe {
            (self.cancel)(
                self.state,
                microphone.as_ptr(),
                render.as_ptr(),
                residual.as_mut_ptr(),
            );
        }
        Ok(residual)
    }

    pub fn reset(&mut self) {
        unsafe { (self.reset)(self.state) };
    }
}

impl Drop for SpeexDspEcho {
    fn drop(&mut self) {
        unsafe {
            (self.destroy)(self.state);
            #[cfg(any(test, feature = "speexdsp-dynamic-test"))]
            if self.dynamic {
                dlclose(self.handle);
            }
        }
    }
}

#[cfg(any(test, feature = "speexdsp-dynamic-test"))]
unsafe fn symbol<T: Copy>(handle: *mut c_void, name: &CStr) -> Option<T> {
    let pointer = unsafe { dlsym(handle, name.as_ptr()) };
    if pointer.is_null() {
        None
    } else if std::mem::size_of::<T>() != std::mem::size_of::<*mut c_void>() {
        None
    } else {
        Some(unsafe { std::mem::transmute_copy(&pointer) })
    }
}

/// Render/capture timeline with a fixed, explicitly configurable hardware
/// delay. Delay estimation is intentionally a separate offline operation;
/// silently guessing in the live owner would make a bad AEC look healthy.
pub struct RenderAecEngine {
    config: AlignmentConfig,
    resampler: Resampler24To16,
    render_history: VecDeque<i16>,
    render_base_sample: u64,
    render_total_samples: u64,
    capture_total_samples: u64,
    expected_sequence: Option<u64>,
    last_render_at: Option<Instant>,
    baseline_head_delta: Option<i64>,
    fail_closed: Option<FailClosedReason>,
    metrics: AecMetrics,
}

impl RenderAecEngine {
    pub fn new(config: AlignmentConfig) -> Self {
        Self {
            config,
            resampler: Resampler24To16::new(),
            render_history: VecDeque::with_capacity(config.history_samples()),
            render_base_sample: 0,
            render_total_samples: 0,
            capture_total_samples: 0,
            expected_sequence: None,
            last_render_at: None,
            baseline_head_delta: None,
            fail_closed: None,
            metrics: AecMetrics::default(),
        }
    }

    pub fn config(&self) -> AlignmentConfig {
        self.config
    }

    pub fn metrics(&self) -> AecMetrics {
        self.metrics
    }

    pub fn fail_closed_reason(&self) -> Option<FailClosedReason> {
        self.fail_closed
    }

    pub fn is_valid(&self) -> bool {
        self.fail_closed.is_none()
    }

    pub fn render_active(&self, now: Instant) -> bool {
        self.last_render_at
            .is_some_and(|last| now.duration_since(last) <= Duration::from_millis(self.config.timeout_ms))
    }

    /// Once a render packet has been observed, the owner stays in the AEC
    /// session until an explicit session reset. This prevents a timed-out
    /// render stream from silently falling back to raw microphone PCM.
    pub fn render_session_seen(&self) -> bool {
        self.last_render_at.is_some()
    }

    pub fn invalidate(&mut self, reason: FailClosedReason) {
        self.fail_closed = Some(reason);
    }

    pub fn reset_session(&mut self) {
        self.resampler.reset();
        self.render_history.clear();
        self.render_base_sample = 0;
        self.render_total_samples = 0;
        self.capture_total_samples = 0;
        self.expected_sequence = None;
        self.last_render_at = None;
        self.baseline_head_delta = None;
        self.fail_closed = None;
    }

    pub fn ingest_packet(&mut self, packet: &[u8], now: Instant) -> Result<(), RenderPacketError> {
        let parsed = match parse_render_packet(packet) {
            Ok(parsed) => parsed,
            Err(error) => {
                self.metrics.packets_rejected = self.metrics.packets_rejected.saturating_add(1);
                self.fail_closed = Some(FailClosedReason::InvalidPacket);
                return Err(error);
            }
        };
        // A START frame opens a new render timeline.  Accepting a later
        // sequence as its first packet would hide a lost prefix and let the
        // AEC align against an incomplete reference.  Keep the session
        // fail-closed until the client explicitly restarts it with START.
        if self.expected_sequence.is_none() && parsed.sequence != 1 {
            self.metrics.packets_rejected = self.metrics.packets_rejected.saturating_add(1);
            self.metrics.sequence_gaps = self.metrics.sequence_gaps.saturating_add(1);
            self.fail_closed = Some(FailClosedReason::SequenceGap);
            return Err(RenderPacketError::InvalidSequence);
        }
        self.metrics.packets_received = self.metrics.packets_received.saturating_add(1);
        if let Some(expected) = self.expected_sequence {
            if parsed.sequence != expected {
                self.metrics.sequence_gaps = self.metrics.sequence_gaps.saturating_add(1);
                self.fail_closed = Some(FailClosedReason::SequenceGap);
            }
        }
        self.expected_sequence = parsed.sequence.checked_add(1);
        let resampled = self.resampler.push_pcm(parsed.pcm);
        for sample in resampled {
            self.render_history.push_back(sample);
            self.render_total_samples = self.render_total_samples.saturating_add(1);
        }
        while self.render_history.len() > self.config.history_samples() {
            let _ = self.render_history.pop_front();
            self.render_base_sample = self.render_base_sample.saturating_add(1);
        }
        self.last_render_at = Some(now);
        Ok(())
    }

    pub fn process_capture_frame(&mut self, capture: &[i16], now: Instant) -> Option<AlignedFrame> {
        if capture.len() != FRAME_SAMPLES {
            self.metrics.frames_rejected = self.metrics.frames_rejected.saturating_add(1);
            self.fail_closed = Some(FailClosedReason::InvalidPacket);
            return None;
        }
        let capture_start = self.capture_total_samples;
        self.capture_total_samples = self
            .capture_total_samples
            .saturating_add(FRAME_SAMPLES as u64);
        if self.last_render_at.is_none() {
            self.metrics.frames_rejected = self.metrics.frames_rejected.saturating_add(1);
            self.fail_closed.get_or_insert(FailClosedReason::NoRender);
            return None;
        }
        if self
            .last_render_at
            .is_some_and(|last| now.duration_since(last) > Duration::from_millis(self.config.timeout_ms))
        {
            self.metrics.stale_render_events = self.metrics.stale_render_events.saturating_add(1);
            self.metrics.frames_rejected = self.metrics.frames_rejected.saturating_add(1);
            self.fail_closed = Some(FailClosedReason::StaleRender);
            return None;
        }
        if let Some(expected_delta) = self.baseline_head_delta {
            let current_delta = self.render_total_samples as i64 - self.capture_total_samples as i64;
            if (current_delta - expected_delta).abs() > self.config.drift_tolerance_samples() {
                self.metrics.drift_events = self.metrics.drift_events.saturating_add(1);
                self.metrics.frames_rejected = self.metrics.frames_rejected.saturating_add(1);
                self.fail_closed = Some(FailClosedReason::Drift);
                return None;
            }
        }
        let delay = self.config.delay_samples();
        let Some(render_start) = capture_start.checked_sub(delay) else {
            self.metrics.frames_rejected = self.metrics.frames_rejected.saturating_add(1);
            return None;
        };
        let render_end = render_start.saturating_add(FRAME_SAMPLES as u64);
        if render_start < self.render_base_sample || render_end > self.render_total_samples {
            self.metrics.render_underflow_events = self.metrics.render_underflow_events.saturating_add(1);
            self.metrics.frames_rejected = self.metrics.frames_rejected.saturating_add(1);
            self.fail_closed.get_or_insert(FailClosedReason::RenderUnderflow);
            return None;
        }
        if !self.is_valid() {
            self.metrics.frames_rejected = self.metrics.frames_rejected.saturating_add(1);
            return None;
        }
        if self.baseline_head_delta.is_none() {
            self.baseline_head_delta = Some(
                self.render_total_samples as i64 - self.capture_total_samples as i64,
            );
        }
        let offset = (render_start - self.render_base_sample) as usize;
        let render = self
            .render_history
            .iter()
            .skip(offset)
            .take(FRAME_SAMPLES)
            .copied()
            .collect::<Vec<_>>();
        if render.len() != FRAME_SAMPLES {
            self.metrics.frames_rejected = self.metrics.frames_rejected.saturating_add(1);
            self.fail_closed = Some(FailClosedReason::RenderUnderflow);
            return None;
        }
        self.metrics.frames_aligned = self.metrics.frames_aligned.saturating_add(1);
        Some(AlignedFrame {
            capture: capture.to_vec(),
            render,
        })
    }

    /// Record measurements produced by an actual downstream DSP backend.
    /// This does not make the owner advertise AEC; the capability gate remains
    /// outside this offline candidate until the backend and device tests pass.
    pub fn record_dsp_measurement(
        &mut self,
        erle_db: f32,
        near_end_energy_ratio: f32,
    ) -> bool {
        if !self.is_valid() || !erle_db.is_finite() || !near_end_energy_ratio.is_finite() {
            return false;
        }
        self.metrics.last_erle_db = Some(erle_db);
        self.metrics.near_end_energy_ratio = Some(near_end_energy_ratio);
        true
    }
}

/// Compute an ERLE measurement from a real DSP residual. The caller must pass
/// the actual pre-cancellation microphone frame and residual; this helper does
/// not synthesize or claim cancellation.
pub fn measure_erle_db(microphone: &[i16], residual: &[i16]) -> Option<f32> {
    if microphone.is_empty() || microphone.len() != residual.len() {
        return None;
    }
    let input_energy: f64 = microphone
        .iter()
        .map(|sample| f64::from(*sample) * f64::from(*sample))
        .sum();
    let residual_energy: f64 = residual
        .iter()
        .map(|sample| f64::from(*sample) * f64::from(*sample))
        .sum();
    if input_energy <= 0.0 || residual_energy <= 0.0 {
        return None;
    }
    Some((10.0 * (input_energy / residual_energy).log10()) as f32)
}

/// Measure preservation of an independently supplied near-end reference. It
/// is intentionally a metric only; passing this check never enables AEC.
pub fn measure_near_end_energy_ratio(near_end: &[i16], residual: &[i16]) -> Option<f32> {
    if near_end.is_empty() || near_end.len() != residual.len() {
        return None;
    }
    let near_energy: f64 = near_end
        .iter()
        .map(|sample| f64::from(*sample) * f64::from(*sample))
        .sum();
    let residual_energy: f64 = residual
        .iter()
        .map(|sample| f64::from(*sample) * f64::from(*sample))
        .sum();
    if near_energy <= 0.0 {
        return None;
    }
    Some((residual_energy / near_energy) as f32)
}

/// Estimate a candidate delay from an offline calibration capture. This is a
/// bounded normalized cross-correlation helper; live operation must pin the
/// resulting value in AlignmentConfig and reject drift beyond its tolerance.
pub fn estimate_delay_samples(
    reference: &[i16],
    capture: &[i16],
    min_delay_samples: usize,
    max_delay_samples: usize,
) -> Option<usize> {
    if reference.is_empty() || capture.is_empty() || min_delay_samples > max_delay_samples {
        return None;
    }
    let mut best: Option<(usize, f64)> = None;
    for delay in min_delay_samples..=max_delay_samples {
        if delay >= capture.len() {
            break;
        }
        let count = reference.len().min(capture.len() - delay);
        if count < FRAME_SAMPLES {
            continue;
        }
        let mut dot = 0.0f64;
        let mut ref_energy = 0.0f64;
        let mut capture_energy = 0.0f64;
        for index in 0..count {
            let left = f64::from(reference[index]);
            let right = f64::from(capture[index + delay]);
            dot += left * right;
            ref_energy += left * left;
            capture_energy += right * right;
        }
        if ref_energy <= 0.0 || capture_energy <= 0.0 {
            continue;
        }
        let score = dot.abs() / (ref_energy * capture_energy).sqrt();
        if best.map(|(_, current)| score > current).unwrap_or(true) {
            best = Some((delay, score));
        }
    }
    best.map(|(delay, _)| delay)
}

/// Private owner-side FVRR1 endpoint. The socket is non-blocking and mode
/// 0600; packet loss is handled as a fail-closed sequence gap by the engine.
pub struct RenderReferenceReceiver {
    socket: UnixDatagram,
    path: String,
    packet_buffer: Vec<u8>,
    capture_pending: Vec<u8>,
    dsp: Option<SpeexDspEcho>,
    session_started: bool,
    pub engine: RenderAecEngine,
}

fn load_dsp() -> Option<SpeexDspEcho> {
    #[cfg(feature = "speexdsp-static")]
    {
        // Static mode is the only supported production candidate mode.
        unsafe { SpeexDspEcho::load_static(FRAME_SAMPLES, 8_000).ok() }
    }
    #[cfg(not(feature = "speexdsp-static"))]
    {
        // The production/default binary never treats a system .so as proof.
        // Use speexdsp-static with a verified archive for an actual target
        // candidate; host-only dynamic loading is test-only below.
        None
    }
}

impl RenderReferenceReceiver {
    pub fn bind(path: &str, config: AlignmentConfig) -> io::Result<Self> {
        match std::fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_socket() => std::fs::remove_file(path)?,
            Ok(_) => {
                return Err(io::Error::new(
                    ErrorKind::AlreadyExists,
                    "render-reference path exists and is not a socket",
                ));
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let socket = UnixDatagram::bind(path)?;
        std::fs::set_permissions(path, Permissions::from_mode(0o600))?;
        socket.set_nonblocking(true)?;
        Ok(Self {
            socket,
            path: path.to_string(),
            // Keep one sentinel byte so recv() cannot silently accept a
            // datagram that was truncated at the maximum packet length.
            packet_buffer: vec![0u8; MAX_RENDER_PACKET_BYTES + 1],
            capture_pending: Vec::with_capacity(FRAME_SAMPLES * 2 * 2),
            // Absence of the official fixed-point library is intentionally
            // tolerated while idle; playback-time use then fails closed.
            dsp: load_dsp(),
            session_started: false,
            engine: RenderAecEngine::new(config),
        })
    }

    pub fn poll(&mut self, now: Instant) -> usize {
        let mut accepted = 0usize;
        loop {
            match self.socket.recv(&mut self.packet_buffer) {
                Ok(size) => {
                    // Observe a bounded copy before existing validation. The
                    // probe independently rejects malformed/sequence-gap data;
                    // it cannot affect this receiver or the forwarding gate.
                    #[cfg(feature = "acoustic-probe")]
                    crate::acoustic_probe::tap_render_datagram(&self.packet_buffer[..size]);
                    if size >= self.packet_buffer.len() {
                        self.packet_buffer.fill(0);
                        self.engine.invalidate(FailClosedReason::InvalidPacket);
                        continue;
                    }
                    if self.packet_buffer[..size] == FVRR1_START[..] {
                        self.reset_session();
                        self.session_started = true;
                        continue;
                    }
                    if self.packet_buffer[..size] == FVRR1_STOP[..] {
                        self.reset_session();
                        self.session_started = false;
                        continue;
                    }
                    if !self.session_started {
                        self.engine.invalidate(FailClosedReason::InvalidPacket);
                        continue;
                    }
                    if self.engine.ingest_packet(&self.packet_buffer[..size], now).is_ok() {
                        accepted = accepted.saturating_add(1);
                    }
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
        self.packet_buffer.fill(0);
        accepted
    }

    pub fn reset_session(&mut self) {
        self.capture_pending.fill(0);
        self.capture_pending.clear();
        self.engine.reset_session();
        if let Some(dsp) = self.dsp.as_mut() {
            dsp.reset();
        }
    }

    pub fn render_session_started(&self) -> bool {
        self.session_started
    }

    /// Process the bounded shared capture stream only while a fresh render
    /// reference is active. Every complete frame must go through the official
    /// SpeexDSP FFI; if it is absent, unhealthy, or not aligned, no raw
    /// capture is returned to the caller.
    pub fn process_capture_for_broadcast(
        &mut self,
        pcm: &[u8],
        now: Instant,
    ) -> Result<Vec<Vec<u8>>, FailClosedReason> {
        if !self.session_started || !self.engine.render_session_seen() {
            self.capture_pending.fill(0);
            self.capture_pending.clear();
            return Err(FailClosedReason::NoRender);
        }
        if !self.engine.render_active(now) {
            self.capture_pending.fill(0);
            self.capture_pending.clear();
            self.engine.invalidate(FailClosedReason::StaleRender);
            return Err(FailClosedReason::StaleRender);
        }
        if self.dsp.is_none() {
            self.capture_pending.fill(0);
            self.capture_pending.clear();
            self.engine.metrics.dsp_failures = self.engine.metrics.dsp_failures.saturating_add(1);
            self.engine.invalidate(FailClosedReason::DspUnavailable);
            return Err(FailClosedReason::DspUnavailable);
        }
        if pcm.is_empty() || pcm.len() % 2 != 0 {
            self.engine.invalidate(FailClosedReason::InvalidPacket);
            return Err(FailClosedReason::InvalidPacket);
        }
        let max_pending = FRAME_SAMPLES * 2 * 4;
        if self.capture_pending.len().saturating_add(pcm.len()) > max_pending {
            self.capture_pending.fill(0);
            self.capture_pending.clear();
            self.engine.invalidate(FailClosedReason::InvalidPacket);
            return Err(FailClosedReason::InvalidPacket);
        }
        self.capture_pending.extend_from_slice(pcm);
        let frame_bytes = FRAME_SAMPLES * 2;
        let mut residual_frames = Vec::new();
        while self.capture_pending.len() >= frame_bytes {
            let mut frame_pcm = self.capture_pending.drain(..frame_bytes).collect::<Vec<_>>();
            let mut capture = Vec::with_capacity(FRAME_SAMPLES);
            for sample in frame_pcm.chunks_exact(2) {
                capture.push(i16::from_le_bytes([sample[0], sample[1]]));
            }
            let Some(aligned) = self.engine.process_capture_frame(&capture, now) else {
                frame_pcm.fill(0);
                capture.fill(0);
                let reason = self.engine.fail_closed_reason().unwrap_or(FailClosedReason::RenderUnderflow);
                self.capture_pending.fill(0);
                self.capture_pending.clear();
                return Err(reason);
            };
            let residual = match self.dsp.as_mut().and_then(|dsp| dsp.process(&aligned.capture, &aligned.render).ok()) {
                Some(residual) => residual,
                None => {
                    frame_pcm.fill(0);
                    self.engine.metrics.dsp_failures = self.engine.metrics.dsp_failures.saturating_add(1);
                    self.engine.invalidate(FailClosedReason::DspUnavailable);
                    self.capture_pending.fill(0);
                    self.capture_pending.clear();
                    return Err(FailClosedReason::DspUnavailable);
                }
            };
            let mut residual_pcm = Vec::with_capacity(frame_bytes);
            for sample in residual {
                residual_pcm.extend_from_slice(&sample.to_le_bytes());
            }
            residual_frames.push(residual_pcm);
            frame_pcm.fill(0);
        }
        Ok(residual_frames)
    }
}

impl Drop for RenderReferenceReceiver {
    fn drop(&mut self) {
        self.packet_buffer.fill(0);
        self.capture_pending.fill(0);
        let _ = std::fs::remove_file(&self.path);
    }

}

impl Drop for RenderAecEngine {
    fn drop(&mut self) {
        for sample in &mut self.render_history {
            *sample = 0;
        }
        self.render_history.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet(sequence: u64, samples: usize) -> Vec<u8> {
        let mut output = Vec::with_capacity(FVRR1_MAGIC.len() + 12 + samples * 2);
        output.extend_from_slice(FVRR1_MAGIC);
        output.extend_from_slice(&sequence.to_le_bytes());
        output.extend_from_slice(&((samples * 2) as u32).to_le_bytes());
        for sample in 0..samples {
            output.extend_from_slice(&(sample as i16).to_le_bytes());
        }
        output
    }

    #[test]
    fn parser_rejects_bad_magic_odd_pcm_and_trailing_bytes() {
        let mut bad_magic = packet(1, 480);
        bad_magic[0] = b'X';
        assert_eq!(parse_render_packet(&bad_magic), Err(RenderPacketError::InvalidMagic));
        let mut odd = packet(1, 480);
        odd[FVRR1_MAGIC.len() + 8..FVRR1_MAGIC.len() + 12]
            .copy_from_slice(&3u32.to_le_bytes());
        assert_eq!(parse_render_packet(&odd), Err(RenderPacketError::InvalidPcm));
        let mut trailing = packet(1, 480);
        trailing.push(0);
        assert_eq!(parse_render_packet(&trailing), Err(RenderPacketError::InvalidLength));
    }

    #[test]
    fn resampling_24k_to_16k_keeps_expected_frame_size() {
        let mut resampler = Resampler24To16::new();
        let pcm = vec![0u8; 480 * 2];
        let output = resampler.push_pcm(&pcm);
        assert_eq!(output.len(), 320);
    }

    #[test]
    fn sequence_gap_fails_closed_and_does_not_return_aligned_frames() {
        let mut engine = RenderAecEngine::new(AlignmentConfig {
            delay_ms: 0,
            ..AlignmentConfig::default()
        });
        let now = Instant::now();
        engine.ingest_packet(&packet(1, 480), now).expect("first packet");
        engine.ingest_packet(&packet(3, 480), now).expect("packet is parsed");
        assert_eq!(engine.fail_closed_reason(), Some(FailClosedReason::SequenceGap));
        assert!(engine.process_capture_frame(&[0; FRAME_SAMPLES], now).is_none());
        assert_eq!(engine.metrics().sequence_gaps, 1);
    }

    #[test]
    fn first_pcm_after_start_must_be_sequence_one() {
        let path = format!(
            "/tmp/family-voice-render-aec-test-{}-{}.sock",
            std::process::id(),
            now_nanos(),
        );
        let mut receiver = RenderReferenceReceiver::bind(&path, AlignmentConfig::default())
            .expect("bind private candidate endpoint");
        let sender = UnixDatagram::unbound().expect("sender");
        sender.send_to(FVRR1_START, &path).expect("start datagram");
        assert_eq!(receiver.poll(Instant::now()), 0);
        sender.send_to(&packet(2, 480), &path).expect("non-one PCM datagram");
        assert_eq!(receiver.poll(Instant::now()), 0);
        assert!(receiver.render_session_started());
        assert!(!receiver.engine.render_session_seen());
        assert_eq!(receiver.engine.fail_closed_reason(), Some(FailClosedReason::SequenceGap));
        drop(sender);
        drop(receiver);
    }

    #[test]
    fn explicit_stop_resets_session_for_next_sequence_one() {
        let mut engine = RenderAecEngine::new(AlignmentConfig::default());
        let now = Instant::now();
        engine.ingest_packet(&packet(1, 480), now).expect("first packet");
        assert!(engine.render_session_seen());
        engine.reset_session();
        engine.ingest_packet(&packet(1, 480), now).expect("new session");
        assert!(engine.is_valid());
        assert_eq!(engine.metrics().sequence_gaps, 0);
    }

    #[test]
    fn stop_datagram_clears_render_session_and_adaptation_state() {
        let path = format!(
            "/tmp/family-voice-render-aec-test-{}-{}.sock",
            std::process::id(),
            now_nanos(),
        );
        let mut receiver = RenderReferenceReceiver::bind(&path, AlignmentConfig::default())
            .expect("bind private candidate endpoint");
        let now = Instant::now();
        let sender = UnixDatagram::unbound().expect("sender");
        sender.send_to(FVRR1_START, &path).expect("start datagram");
        assert_eq!(receiver.poll(now), 0);
        assert!(receiver.render_session_started());
        sender.send_to(&packet(1, 480), &path).expect("first PCM datagram");
        assert_eq!(receiver.poll(now), 1);
        assert!(receiver.engine.render_session_seen());
        sender.send_to(FVRR1_STOP, &path).expect("stop datagram");
        assert_eq!(receiver.poll(now), 0);
        assert!(!receiver.render_session_started());
        assert!(!receiver.engine.render_session_seen());
        assert!(receiver.engine.is_valid());
        drop(sender);
        drop(receiver);
    }

    #[test]
    fn pcm_before_start_is_rejected_and_cannot_open_raw_fallback() {
        let path = format!(
            "/tmp/family-voice-render-aec-test-{}-{}.sock",
            std::process::id(),
            now_nanos(),
        );
        let mut receiver = RenderReferenceReceiver::bind(&path, AlignmentConfig::default())
            .expect("bind private candidate endpoint");
        let sender = UnixDatagram::unbound().expect("sender");
        sender.send_to(&packet(1, 480), &path).expect("PCM datagram");
        assert_eq!(receiver.poll(Instant::now()), 0);
        assert!(!receiver.render_session_started());
        assert_eq!(receiver.engine.fail_closed_reason(), Some(FailClosedReason::InvalidPacket));
        drop(sender);
        drop(receiver);
    }

    fn now_nanos() -> u128 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos()
    }

    #[test]
    fn stale_render_and_drift_are_fail_closed() {
        let mut engine = RenderAecEngine::new(AlignmentConfig {
            delay_ms: 0,
            timeout_ms: 1,
            ..AlignmentConfig::default()
        });
        let now = Instant::now();
        engine.ingest_packet(&packet(1, 480), now).expect("packet");
        assert!(engine
            .process_capture_frame(&[0; FRAME_SAMPLES], now + Duration::from_millis(2))
            .is_none());
        assert_eq!(engine.fail_closed_reason(), Some(FailClosedReason::StaleRender));
    }

    #[test]
    fn erle_and_near_end_metrics_are_measurements_only() {
        let microphone = vec![100i16; FRAME_SAMPLES];
        let residual = vec![10i16; FRAME_SAMPLES];
        let near_end = vec![10i16; FRAME_SAMPLES];
        let erle = measure_erle_db(&microphone, &residual).expect("ERLE");
        assert!(erle > 19.0 && erle < 21.0);
        let ratio = measure_near_end_energy_ratio(&near_end, &residual).expect("near end");
        assert!((ratio - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn offline_delay_estimator_finds_known_alignment() {
        let reference = (0..FRAME_SAMPLES * 2)
            .map(|index| if index % 7 == 0 { 1000 } else { -300 })
            .collect::<Vec<_>>();
        let delay = 37;
        let mut capture = vec![0i16; delay];
        capture.extend_from_slice(&reference);
        assert_eq!(estimate_delay_samples(&reference, &capture, 0, 80), Some(delay));
    }

    #[test]
    #[ignore = "requires an official fixed-point libspeexdsp"]
    fn official_speex_handles_synthetic_echo_and_double_talk() {
        #[cfg(feature = "speexdsp-static")]
        let mut dsp = unsafe {
            SpeexDspEcho::load_static(FRAME_SAMPLES, 8_000)
                .expect("official SpeexDSP fixed-point library")
        };
        #[cfg(not(feature = "speexdsp-static"))]
        let mut dsp = {
            let path = std::env::var("FVS_SPEEXDSP_LIBRARY").expect("FVS_SPEEXDSP_LIBRARY");
            let path = std::ffi::CString::new(path).expect("library path");
            unsafe {
                SpeexDspEcho::load(path.as_c_str(), FRAME_SAMPLES, 8_000)
                    .expect("official SpeexDSP fixed-point library")
            }
        };
        let mut random = 0x1234_5678u32;
        let mut input_energy = 0.0f64;
        let mut residual_energy = 0.0f64;
        // First adapt and measure with far-end echo only. Mixing near-end
        // speech into ERLE would measure double-talk energy rather than echo
        // attenuation and can make a healthy canceller appear worse.
        for frame_index in 0..500 {
            let mut render = [0i16; FRAME_SAMPLES];
            let mut microphone = [0i16; FRAME_SAMPLES];
            for sample in 0..FRAME_SAMPLES {
                random = random.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let far = ((random >> 16) as i16) / 8;
                render[sample] = far;
                microphone[sample] = ((i32::from(far) * 3) / 5) as i16;
            }
            let residual = dsp.process(&microphone, &render).expect("Speex frame");
            if frame_index >= 350 {
                input_energy += microphone
                    .iter()
                    .map(|sample| f64::from(*sample) * f64::from(*sample))
                    .sum::<f64>();
                residual_energy += residual
                    .iter()
                    .map(|sample| f64::from(*sample) * f64::from(*sample))
                    .sum::<f64>();
            }
        }
        let erle = 10.0 * (input_energy / residual_energy).log10();
        assert!(erle > 6.0, "synthetic ERLE too low: {erle:.2} dB");

        // Then add an independent near-end waveform. The residual must retain
        // its energy and correlation instead of treating the user as echo.
        let mut near_energy = 0.0f64;
        let mut output_energy = 0.0f64;
        let mut near_output_dot = 0.0f64;
        for frame_index in 0..80 {
            let mut render = [0i16; FRAME_SAMPLES];
            let mut microphone = [0i16; FRAME_SAMPLES];
            let mut near_end = [0i16; FRAME_SAMPLES];
            for sample in 0..FRAME_SAMPLES {
                random = random.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let far = ((random >> 16) as i16) / 8;
                let phase = ((frame_index * FRAME_SAMPLES + sample) % 64) as i32 - 32;
                let near = (phase * 60) as i16;
                render[sample] = far;
                near_end[sample] = near;
                microphone[sample] = (((i32::from(far) * 3) / 5) + i32::from(near))
                    .clamp(i16::MIN as i32, i16::MAX as i32) as i16;
            }
            let residual = dsp.process(&microphone, &render).expect("Speex double-talk frame");
            for (near, output) in near_end.iter().zip(residual.iter()) {
                let near = f64::from(*near);
                let output = f64::from(*output);
                near_energy += near * near;
                output_energy += output * output;
                near_output_dot += near * output;
            }
        }
        let near_ratio = output_energy / near_energy;
        let correlation = near_output_dot / (near_energy * output_energy).sqrt();
        assert!(near_ratio.is_finite() && near_ratio > 0.2 && near_ratio < 5.0);
        assert!(correlation > 0.5, "near-end correlation too low: {correlation:.3}");
    }
}
