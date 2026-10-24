//! Memory-only observations, NOT AEC/ERLE or final DAC-reference acceptance.
//!
//! Packet timestamps identify the FIRST sample on one relative 48 kHz timeline.
//! Never silently substitute callback arrival for a sample timestamp: label that
//! explicitly with `TimingBasis::CallbackArrival`. No clock synchronization is
//! inferred here. Timestamp uncertainty is metadata, not corrected latency.
//!
//! Each 1024 ms window has 2048 non-overlapping 0.5 ms boxcar means. This is a
//! rectangular smoothing/decimation diagnostic (24/8/12 samples per bin), NOT a
//! high-rejection antialias resampler; high-frequency aliases and periodic input
//! can produce ambiguous candidates. Positive lag means capture occurs AFTER
//! software render: corr(capture[i + lag], render[i]). Scan -512..=512 bins using
//! the SAME 1024 render bins (indices 512..1536) at every lag. Resolution is 0.5 ms,
//! search range +/-256 ms; an edge maximum is unavailable, not a delay estimate.
//! Full stream coverage is required, so gaps, malformed input and missing bins
//! cannot silently shrink the comparison or become zero-valued measurements.
//!
//! Six streams retain only fixed-sized bin sums/counts and native-level scalar
//! statistics (<128 KiB); reports expose no PCM. Push work is linear in at most
//! one window of packet samples. Report work is bounded by 5*1025*1024 pairs.
//! Run collection/reporting on a bounded worker, never the realtime callback.
//! Reset and Drop explicitly volatile-zero retained audio-derived bin values;
//! this is best-effort process-memory hygiene, not a secure-memory guarantee.

use std::sync::atomic::{compiler_fence, Ordering};

pub const BIN_TICKS_48K: i64 = 24;
pub const WINDOW_BINS: usize = 2048;
pub const WINDOW_TICKS_48K: i64 = WINDOW_BINS as i64 * BIN_TICKS_48K;
pub const MAX_LAG_BINS: i32 = 512;
pub const LAG_QUANTUM_US: u32 = 500;
pub const COMPARISON_SAMPLES: usize = 1024;
/// Diagnostic floor only; not a calibrated acoustic acceptance threshold.
pub const MIN_ABS_CORRELATION: f64 = 0.2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimingBasis {
    SharedSampleClock,
    MonotonicEstimate,
    CallbackArrival,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timing {
    pub basis: TimingBasis,
    /// Conservative uncertainty of RELATIVE alignment across all streams.
    /// None explicitly means unknown, including scheduling and playback queues.
    pub uncertainty_ticks_48k: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputError {
    InvalidWindowStart,
    EmptyPacket,
    InvalidLength,
    PacketTooLarge,
    InvalidTimestamp,
    Discontinuity,
    StreamInvalidated,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unavailable {
    MissingStream,
    IncompleteWindow,
    Discontinuity,
    InvalidInput,
    SilenceOrConstant,
    LowCorrelation,
    PeakAtSearchBoundary,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LevelStats {
    pub sample_count: u64,
    pub zero_count: u64,
    pub nonzero_count: u64,
    /// Exact PCM container extrema, not a claim about analog clipping or S32
    /// effective bit depth. Left-aligned narrower audio may never hit i32::MAX.
    pub clipped_count: u64,
    /// Sum of squared samples normalized by 2^31 (raw) or 2^15 (S16).
    pub sum_squares: f64,
    pub rms: Option<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChannelReport {
    pub levels: LevelStats,
    pub complete_bins: usize,
    /// Level counters remain observations of received samples when unavailable.
    pub unavailable: Option<Unavailable>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Correlation {
    pub lag_bins: i32,
    pub lag_us: i32,
    /// Signed, mean-subtracted normalized coefficient at the largest absolute
    /// peak. Inverted channels therefore have a negative coefficient.
    pub coefficient: f64,
    pub compared_samples: usize,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProbeReport {
    pub start_tick_48k: i64,
    pub timing: Timing,
    pub raw: [ChannelReport; 4],
    pub processed: ChannelReport,
    pub software_render: ChannelReport,
    pub raw_vs_render: [Result<Correlation, Unavailable>; 4],
    pub processed_vs_render: Result<Correlation, Unavailable>,
}

struct Lane {
    sums: [f64; WINDOW_BINS],
    counts: [u16; WINDOW_BINS],
    levels: LevelStats,
    expected_tick: Option<i64>,
    invalid: Option<Unavailable>,
}

impl Lane {
    fn new() -> Self {
        Self {
            sums: [0.0; WINDOW_BINS],
            counts: [0; WINDOW_BINS],
            levels: LevelStats::default(),
            expected_tick: None,
            invalid: None,
        }
    }

    fn wipe(&mut self) {
        for value in &mut self.sums {
            // SAFETY: each pointer refers to a live, exclusively borrowed f64.
            unsafe { std::ptr::write_volatile(value, 0.0) };
        }
        self.counts.fill(0);
        self.levels = LevelStats::default();
        self.expected_tick = None;
        self.invalid = None;
        compiler_fence(Ordering::SeqCst);
    }

    fn add(&mut self, bin: usize, sample: i64, scale: f64, clipped: bool) {
        let value = sample as f64 / scale;
        self.sums[bin] += value;
        self.counts[bin] += 1;
        self.levels.sample_count += 1;
        self.levels.zero_count += u64::from(sample == 0);
        self.levels.nonzero_count += u64::from(sample != 0);
        self.levels.clipped_count += u64::from(clipped);
        self.levels.sum_squares += value * value;
    }

    fn report(&self, per_bin: u16) -> ChannelReport {
        let complete_bins = self.counts.iter().filter(|&&n| n == per_bin).count();
        let mut levels = self.levels;
        levels.rms = (levels.sample_count != 0)
            .then(|| (levels.sum_squares / levels.sample_count as f64).sqrt());
        ChannelReport {
            levels,
            complete_bins,
            unavailable: self.invalid.or_else(|| {
                if levels.sample_count == 0 {
                    Some(Unavailable::MissingStream)
                } else if complete_bins != WINDOW_BINS {
                    Some(Unavailable::IncompleteWindow)
                } else {
                    None
                }
            }),
        }
    }
}

pub struct ProbeWindow {
    start_tick: i64,
    timing: Timing,
    lanes: [Lane; 6],
}

impl ProbeWindow {
    pub fn new(start_tick_48k: i64, timing: Timing) -> Result<Self, InputError> {
        Self::validate_start(start_tick_48k)?;
        Ok(Self {
            start_tick: start_tick_48k,
            timing,
            lanes: std::array::from_fn(|_| Lane::new()),
        })
    }

    fn validate_start(tick: i64) -> Result<(), InputError> {
        if tick < 0 || tick % BIN_TICKS_48K != 0 || tick.checked_add(WINDOW_TICKS_48K).is_none() {
            Err(InputError::InvalidWindowStart)
        } else {
            Ok(())
        }
    }

    /// A fresh window also permits new stream timestamp origins. An invalid
    /// reset request leaves the previous state intact.
    pub fn reset(&mut self, start_tick_48k: i64, timing: Timing) -> Result<(), InputError> {
        Self::validate_start(start_tick_48k)?;
        for lane in &mut self.lanes {
            lane.wipe();
        }
        self.start_tick = start_tick_48k;
        self.timing = timing;
        Ok(())
    }

    /// Explicitly invalidate a dropped queue packet/disconnected producer.
    /// Raw channels are indices 0..4, processed=4, software render=5. Invalid
    /// lane indices return false. A raw drop must mark all four raw channels.
    pub fn mark_gap(&mut self, lane: usize) -> bool {
        if let Some(lane) = self.lanes.get_mut(lane) {
            lane.invalid = Some(Unavailable::Discontinuity);
            true
        } else {
            false
        }
    }

    pub fn push_raw(&mut self, tick_48k: i64, pcm_s32le_4ch: &[u8]) -> Result<(), InputError> {
        self.push(tick_48k, pcm_s32le_4ch, 0, 4, 4, 1)
    }

    pub fn push_processed(&mut self, tick_48k: i64, pcm_s16le: &[u8]) -> Result<(), InputError> {
        self.push(tick_48k, pcm_s16le, 4, 1, 2, 3)
    }

    pub fn push_render(&mut self, tick_48k: i64, pcm_s16le: &[u8]) -> Result<(), InputError> {
        self.push(tick_48k, pcm_s16le, 5, 1, 2, 2)
    }

    fn push(
        &mut self,
        tick: i64,
        bytes: &[u8],
        first: usize,
        channels: usize,
        width: usize,
        step: i64,
    ) -> Result<(), InputError> {
        let frame_bytes = channels * width;
        let frames = bytes.len() / frame_bytes;
        let error = if self.lanes[first..first + channels]
            .iter()
            .any(|l| l.invalid.is_some())
        {
            Some(InputError::StreamInvalidated)
        } else if bytes.is_empty() {
            Some(InputError::EmptyPacket)
        } else if bytes.len() % frame_bytes != 0 {
            Some(InputError::InvalidLength)
        } else if frames > (WINDOW_TICKS_48K / step) as usize {
            Some(InputError::PacketTooLarge)
        } else if tick < 0 || tick % step != 0 || tick.checked_add(frames as i64 * step).is_none() {
            Some(InputError::InvalidTimestamp)
        } else if self.lanes[first]
            .expected_tick
            .is_some_and(|expected| tick != expected)
        {
            Some(InputError::Discontinuity)
        } else {
            None
        };
        if let Some(error) = error {
            if error != InputError::StreamInvalidated {
                for lane in &mut self.lanes[first..first + channels] {
                    lane.invalid = Some(if error == InputError::Discontinuity {
                        Unavailable::Discontinuity
                    } else {
                        Unavailable::InvalidInput
                    });
                }
            }
            return Err(error);
        }
        let end = self.start_tick + WINDOW_TICKS_48K;
        for (frame, data) in bytes.chunks_exact(frame_bytes).enumerate() {
            let sample_tick = tick + frame as i64 * step;
            if sample_tick < self.start_tick || sample_tick >= end {
                continue;
            }
            let bin = ((sample_tick - self.start_tick) / BIN_TICKS_48K) as usize;
            for channel in 0..channels {
                let offset = channel * width;
                let (sample, scale, clipped) = if width == 4 {
                    let sample = i32::from_le_bytes(data[offset..offset + 4].try_into().unwrap());
                    (
                        sample as i64,
                        2147483648.0,
                        sample == i32::MIN || sample == i32::MAX,
                    )
                } else {
                    let sample = i16::from_le_bytes(data[offset..offset + 2].try_into().unwrap());
                    (
                        sample as i64,
                        32768.0,
                        sample == i16::MIN || sample == i16::MAX,
                    )
                };
                self.lanes[first + channel].add(bin, sample, scale, clipped);
            }
        }
        for lane in &mut self.lanes[first..first + channels] {
            lane.expected_tick = Some(tick + frames as i64 * step);
        }
        Ok(())
    }

    pub fn report(&self) -> ProbeReport {
        let raw = std::array::from_fn(|i| self.lanes[i].report(24));
        let processed = self.lanes[4].report(8);
        let software_render = self.lanes[5].report(12);
        ProbeReport {
            start_tick_48k: self.start_tick,
            timing: self.timing,
            raw,
            processed,
            software_render,
            raw_vs_render: std::array::from_fn(|i| self.correlate(i, raw[i], software_render)),
            processed_vs_render: self.correlate(4, processed, software_render),
        }
    }

    fn correlate(
        &self,
        capture: usize,
        capture_report: ChannelReport,
        render_report: ChannelReport,
    ) -> Result<Correlation, Unavailable> {
        if let Some(reason) = capture_report.unavailable.or(render_report.unavailable) {
            return Err(reason);
        }
        // Per-lane boxcar scale is constant and cancels in Pearson correlation.
        let x = &self.lanes[capture].sums;
        let y = &self.lanes[5].sums;
        let n = COMPARISON_SAMPLES as f64;
        let mut best: Option<Correlation> = None;
        for lag in -MAX_LAG_BINS..=MAX_LAG_BINS {
            let (mut sx, mut sy, mut sxx, mut syy, mut sxy) = (0.0, 0.0, 0.0, 0.0, 0.0);
            for j in MAX_LAG_BINS as usize..MAX_LAG_BINS as usize + COMPARISON_SAMPLES {
                let a = x[(j as i32 + lag) as usize];
                let b = y[j];
                sx += a;
                sy += b;
                sxx += a * a;
                syy += b * b;
                sxy += a * b;
            }
            let vx = (sxx - sx * sx / n).max(0.0);
            let vy = (syy - sy * sy / n).max(0.0);
            // Relative cancellation guard rejects constant/DC-only arrays.
            if vx <= sxx * 1e-12 || vy <= syy * 1e-12 || vx == 0.0 || vy == 0.0 {
                continue;
            }
            let coefficient = ((sxy - sx * sy / n) / (vx.sqrt() * vy.sqrt())).clamp(-1.0, 1.0);
            if best.is_none_or(|old| coefficient.abs() > old.coefficient.abs()) {
                best = Some(Correlation {
                    lag_bins: lag,
                    lag_us: lag * LAG_QUANTUM_US as i32,
                    coefficient,
                    compared_samples: COMPARISON_SAMPLES,
                });
            }
        }
        let best = best.ok_or(Unavailable::SilenceOrConstant)?;
        if best.lag_bins.abs() == MAX_LAG_BINS {
            Err(Unavailable::PeakAtSearchBoundary)
        } else if best.coefficient.abs() < MIN_ABS_CORRELATION {
            Err(Unavailable::LowCorrelation)
        } else {
            Ok(best)
        }
    }
}

impl Drop for ProbeWindow {
    fn drop(&mut self) {
        for lane in &mut self.lanes {
            lane.wipe();
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DelayChange {
    pub elapsed_ticks_48k: i64,
    /// Difference between two quantized candidate lags; NOT a clock-rate or
    /// acoustic drift measurement. Scheduling can dominate this observation.
    pub delta_lag_us: i32,
    pub timing: Timing,
}

#[derive(Default)]
pub struct DelayTracker {
    previous: Option<ProbeReport>,
}

impl DelayTracker {
    /// Raw0..3 and processed4. Missing/invalid, overlapping/repeated windows or
    /// changed timing quality break the chain; no synthetic zero drift appears.
    pub fn observe(&mut self, report: &ProbeReport) -> [Option<DelayChange>; 5] {
        let mut changes = [None; 5];
        if let Some(previous) = self.previous {
            let elapsed = report.start_tick_48k.checked_sub(previous.start_tick_48k);
            if previous.timing == report.timing && elapsed.is_some_and(|e| e >= WINDOW_TICKS_48K) {
                for (i, change) in changes.iter_mut().enumerate() {
                    let before = if i < 4 {
                        previous.raw_vs_render[i]
                    } else {
                        previous.processed_vs_render
                    };
                    let after = if i < 4 {
                        report.raw_vs_render[i]
                    } else {
                        report.processed_vs_render
                    };
                    if let (Ok(before), Ok(after)) = (before, after) {
                        *change = Some(DelayChange {
                            elapsed_ticks_48k: elapsed.unwrap(),
                            delta_lag_us: after.lag_us - before.lag_us,
                            timing: report.timing,
                        });
                    }
                }
            }
        }
        self.previous = Some(*report);
        changes
    }

    pub fn reset(&mut self) {
        self.previous = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn timing() -> Timing {
        Timing {
            basis: TimingBasis::SharedSampleClock,
            uncertainty_ticks_48k: Some(0),
        }
    }

    fn noise(index: i64, seed: u32) -> i16 {
        let mut x = (index as u32)
            .wrapping_add(seed)
            .wrapping_mul(747796405)
            .wrapping_add(2891336453);
        x = ((x >> ((x >> 28) + 4)) ^ x).wrapping_mul(277803737);
        (((x >> 22) ^ x) % 24001) as i16 - 12000
    }

    fn fixture(start: i64, lag: i32, chunks: usize) -> ProbeWindow {
        let mut window = ProbeWindow::new(start, timing()).unwrap();
        let raw: Vec<u8> = (0..WINDOW_TICKS_48K)
            .flat_map(|tick| {
                let bin = tick / 24;
                let value = noise(bin - lag as i64, 27) as i32;
                [
                    value * 32768,
                    -value * 8192,
                    noise(bin, 543) as i32 * 16384,
                    0,
                ]
                .into_iter()
                .flat_map(i32::to_le_bytes)
            })
            .collect();
        let processed: Vec<u8> = (0..WINDOW_TICKS_48K / 3)
            .flat_map(|frame| (noise(frame / 8 - lag as i64, 27) / 2).to_le_bytes())
            .collect();
        let render: Vec<u8> = (0..WINDOW_TICKS_48K / 2)
            .flat_map(|frame| noise(frame / 12, 27).to_le_bytes())
            .collect();
        // Deliberately different native chunk boundaries, including partial bins.
        for (i, packet) in raw.chunks(chunks * 16).enumerate() {
            window
                .push_raw(start + (i * chunks) as i64, packet)
                .unwrap();
        }
        for (i, packet) in processed.chunks((chunks + 7) * 2).enumerate() {
            window
                .push_processed(start + (i * (chunks + 7) * 3) as i64, packet)
                .unwrap();
        }
        for (i, packet) in render.chunks((chunks + 11) * 2).enumerate() {
            window
                .push_render(start + (i * (chunks + 11) * 2) as i64, packet)
                .unwrap();
        }
        window
    }

    #[test]
    fn delayed_scaled_inverted_and_independent_channels() {
        let report = fixture(0, 37, 71).report();
        for i in [0, 1] {
            let value = report.raw_vs_render[i].unwrap();
            assert_eq!(value.lag_bins, 37);
            assert_eq!(value.lag_us, 18500);
            assert_eq!(value.compared_samples, COMPARISON_SAMPLES);
            assert!(value.coefficient.abs() > 0.999999);
        }
        assert!(report.raw_vs_render[0].unwrap().coefficient > 0.0);
        assert!(report.raw_vs_render[1].unwrap().coefficient < 0.0);
        assert_eq!(report.raw_vs_render[2], Err(Unavailable::LowCorrelation));
        assert_eq!(report.raw_vs_render[3], Err(Unavailable::SilenceOrConstant));
        assert_eq!(report.processed_vs_render.unwrap().lag_bins, 37);
    }

    #[test]
    fn negative_zero_delays_and_arbitrary_chunks_preserve_results() {
        for lag in [-95, 0, 193] {
            let a = fixture(WINDOW_TICKS_48K, lag, 113).report();
            let b = fixture(WINDOW_TICKS_48K, lag, WINDOW_TICKS_48K as usize / 4).report();
            assert_eq!(a, b);
            assert_eq!(a.raw_vs_render[0].unwrap().lag_bins, lag);
        }
    }

    #[test]
    fn boundary_peak_unavailable() {
        for lag in [-MAX_LAG_BINS, MAX_LAG_BINS] {
            assert_eq!(
                fixture(0, lag, 200).report().raw_vs_render[0],
                Err(Unavailable::PeakAtSearchBoundary)
            );
        }
    }

    #[test]
    fn beyond_search_range_does_not_invent_delay() {
        for lag in [-MAX_LAG_BINS - 73, MAX_LAG_BINS + 81] {
            assert!(fixture(0, lag, 83).report().raw_vs_render[0].is_err());
        }
    }

    #[test]
    fn fractional_bin_delay_is_quantized_not_subsample_precision() {
        for (delay_ticks, expected_bins) in [(37 * 24 + 7, 37), (37 * 24 + 17, 38)] {
            let mut window = ProbeWindow::new(0, timing()).unwrap();
            let raw: Vec<u8> = (0..WINDOW_TICKS_48K)
                .flat_map(|tick| {
                    let value = noise((tick - delay_ticks).div_euclid(24), 27) as i32 * 32768;
                    [value, 0, 0, 0].into_iter().flat_map(i32::to_le_bytes)
                })
                .collect();
            let render: Vec<u8> = (0..WINDOW_TICKS_48K / 2)
                .flat_map(|frame| noise(frame / 12, 27).to_le_bytes())
                .collect();
            window.push_raw(0, &raw).unwrap();
            window.push_render(0, &render).unwrap();
            let candidate = window.report().raw_vs_render[0].unwrap();
            assert_eq!(candidate.lag_bins, expected_bins);
            assert_eq!(candidate.lag_us % LAG_QUANTUM_US as i32, 0);
            assert!(candidate.coefficient > 0.9);
        }
    }

    #[test]
    fn missing_silent_constant_and_partial_never_become_zero_delay() {
        let mut window = ProbeWindow::new(0, timing()).unwrap();
        let report = window.report();
        assert_eq!(report.raw[0].levels.rms, None);
        assert_eq!(report.raw_vs_render[0], Err(Unavailable::MissingStream));
        window
            .push_raw(0, &vec![0; WINDOW_TICKS_48K as usize * 16])
            .unwrap();
        assert_eq!(
            window.report().raw_vs_render[0],
            Err(Unavailable::MissingStream)
        );
        window
            .push_render(0, &vec![0; WINDOW_TICKS_48K as usize])
            .unwrap();
        assert_eq!(
            window.report().raw_vs_render[0],
            Err(Unavailable::SilenceOrConstant)
        );
        assert_eq!(window.report().raw[0].levels.rms, Some(0.0));
        window.reset(0, timing()).unwrap();
        window.push_raw(0, &[0; 16]).unwrap();
        assert_eq!(
            window.report().raw[0].unavailable,
            Some(Unavailable::IncompleteWindow)
        );
        window.reset(0, timing()).unwrap();
        window
            .push_raw(0, &vec![1; WINDOW_TICKS_48K as usize * 16])
            .unwrap();
        window
            .push_render(0, &vec![1; WINDOW_TICKS_48K as usize])
            .unwrap();
        assert_eq!(
            window.report().raw_vs_render[0],
            Err(Unavailable::SilenceOrConstant)
        );
    }

    #[test]
    fn level_stats_count_each_native_sample_and_extrema() {
        let mut window = ProbeWindow::new(0, timing()).unwrap();
        let raw: Vec<u8> = [i32::MIN, i32::MAX, 0, 1]
            .into_iter()
            .flat_map(i32::to_le_bytes)
            .collect();
        window.push_raw(0, &raw).unwrap();
        window
            .push_processed(0, &[0, 128, 255, 127, 0, 0, 1, 0])
            .unwrap();
        let report = window.report();
        assert_eq!(report.raw[0].levels.sum_squares, 1.0);
        assert_eq!(report.raw[0].levels.clipped_count, 1);
        assert_eq!(report.raw[1].levels.clipped_count, 1);
        assert_eq!(report.raw[2].levels.zero_count, 1);
        assert_eq!(report.raw[3].levels.nonzero_count, 1);
        assert_eq!(report.processed.levels.sample_count, 4);
        assert_eq!(report.processed.levels.clipped_count, 2);
        assert_eq!(report.processed.levels.zero_count, 1);
        assert_eq!(report.processed.levels.nonzero_count, 3);
    }

    #[test]
    fn malformed_packets_poison_stream_until_reset() {
        let mut window = ProbeWindow::new(0, timing()).unwrap();
        for (bytes, tick, expected) in [
            (&[][..], 0, InputError::EmptyPacket),
            (&[0][..], 0, InputError::InvalidLength),
            (&[0, 0][..], 1, InputError::InvalidTimestamp),
        ] {
            window.reset(0, timing()).unwrap();
            assert_eq!(window.push_render(tick, bytes), Err(expected));
            assert_eq!(
                window.report().software_render.unavailable,
                Some(Unavailable::InvalidInput)
            );
            assert_eq!(
                window.push_render(0, &[0, 0]),
                Err(InputError::StreamInvalidated)
            );
        }
        window.reset(0, timing()).unwrap();
        assert_eq!(
            window.push_raw(0, &vec![0; (WINDOW_TICKS_48K as usize + 1) * 16]),
            Err(InputError::PacketTooLarge)
        );
        window.reset(0, timing()).unwrap();
        assert_eq!(
            window.push_render(i64::MAX - 1, &[0; 4]),
            Err(InputError::InvalidTimestamp)
        );
        assert!(ProbeWindow::new(-24, timing()).is_err());
        assert!(ProbeWindow::new(1, timing()).is_err());
    }

    #[test]
    fn skipped_duplicate_and_reordered_packets_are_gaps() {
        for next in [0, 2, 12] {
            let mut window = ProbeWindow::new(0, timing()).unwrap();
            window.push_render(0, &[0; 8]).unwrap();
            assert_eq!(
                window.push_render(next, &[0; 4]),
                Err(InputError::Discontinuity)
            );
            assert_eq!(
                window.report().software_render.unavailable,
                Some(Unavailable::Discontinuity)
            );
        }
    }

    #[test]
    fn samples_outside_window_do_not_count_and_offset_is_not_arrival_time() {
        let mut window = ProbeWindow::new(24, timing()).unwrap();
        window.push_render(0, &[1; 48]).unwrap();
        assert_eq!(window.report().software_render.levels.sample_count, 12);
        window
            .push_render(48, &vec![1; WINDOW_TICKS_48K as usize])
            .unwrap();
        assert_eq!(
            window.report().software_render.levels.sample_count,
            (WINDOW_TICKS_48K / 2) as u64
        );
        assert_eq!(window.report().software_render.complete_bins, WINDOW_BINS);
    }

    #[test]
    fn reset_wipes_audio_aggregates_and_restores_capacity() {
        let mut window = fixture(0, 31, 64);
        window.reset(WINDOW_TICKS_48K, timing()).unwrap();
        for lane in &window.lanes {
            assert!(lane.sums.iter().all(|x| *x == 0.0));
            assert!(lane.counts.iter().all(|x| *x == 0));
            assert_eq!(lane.levels, LevelStats::default());
        }
        assert!(std::mem::size_of::<ProbeWindow>() < 128 * 1024);
        assert!(!window.mark_gap(6));
        assert!(window.mark_gap(4));
        assert_eq!(
            window.report().processed.unavailable,
            Some(Unavailable::Discontinuity)
        );
    }

    #[test]
    fn drift_is_only_successive_candidate_difference_with_timing_metadata() {
        let mut tracker = DelayTracker::default();
        let a = fixture(0, 20, 101).report();
        let mut b = fixture(WINDOW_TICKS_48K, 23, 101).report();
        assert_eq!(tracker.observe(&a), [None; 5]);
        assert_eq!(tracker.observe(&b)[0].unwrap().delta_lag_us, 1500);
        assert_eq!(tracker.observe(&b), [None; 5]);
        b.start_tick_48k += WINDOW_TICKS_48K;
        b.timing = Timing {
            basis: TimingBasis::CallbackArrival,
            uncertainty_ticks_48k: None,
        };
        assert_eq!(tracker.observe(&b), [None; 5]);
        b.start_tick_48k += WINDOW_TICKS_48K;
        assert_eq!(
            tracker.observe(&b)[0].unwrap().timing.uncertainty_ticks_48k,
            None
        );
        b.raw_vs_render[0] = Err(Unavailable::MissingStream);
        b.start_tick_48k += WINDOW_TICKS_48K;
        assert_eq!(tracker.observe(&b)[0], None);
        tracker.reset();
        assert_eq!(tracker.observe(&a), [None; 5]);
    }

    #[test]
    fn signal_plus_noise_retains_delay() {
        let mut window = fixture(0, 41, 143);
        // Noise at the actual retained-bin representation, deterministic and
        // weaker than the known delayed signal. No real audio is used.
        for (i, sum) in window.lanes[0].sums.iter_mut().enumerate() {
            *sum += noise(i as i64, 885) as f64 / 32768.0 * 0.25;
        }
        let correlation = window.report().raw_vs_render[0].unwrap();
        assert_eq!(correlation.lag_bins, 41);
        assert!(correlation.coefficient > 0.98);
    }
}
