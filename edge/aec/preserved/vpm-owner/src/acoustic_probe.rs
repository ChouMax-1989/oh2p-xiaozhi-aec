//! Passive acoustic probe. Default build excludes this module entirely.
//! Hot-path hooks only try_send bounded copies while a single request is active.
//! Audio-derived buffers never leave this process; only whitelisted scalars do.
//! The observer cannot change VPM status, forwarding, playback, or capability.

#[path = "acoustic_probe_metrics.rs"]
mod metrics;

#[path = "target_probe.rs"]
mod target;

use metrics::{
    ChannelReport, Correlation, ProbeReport, ProbeWindow, Timing, TimingBasis, Unavailable,
};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

const QUEUE_CAPACITY: usize = 32;
const MAX_COLLECTION_EVENTS: u32 = 1024;
const MAX_PCM_BYTES: usize = 16_384;
const MAX_EVENT_BYTES: usize = MAX_PCM_BYTES + 22;
const WINDOW_START: i64 = 19_200; // 400 ms warm-up, not measured as audio delay.
const COLLECTION_LIMIT: Duration = Duration::from_secs(2);
const REQUEST_MEASURE: &[u8] = b"FVAP1 MEASURE\n";
const REQUEST_STATUS: &[u8] = b"FVAP1 STATUS\n";
const REQUEST_DISABLE: &[u8] = b"FVAP1 DISABLE\n";
const RENDER_START: &[u8] = b"FVRR1 START\n";
const RENDER_STOP: &[u8] = b"FVRR1 STOP\n";
const RENDER_MAGIC: &[u8] = b"FVRR1 PCM\n";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Source {
    Raw,
    Processed,
    Render,
}

impl Source {
    fn index(self) -> usize {
        match self {
            Self::Raw => 0,
            Self::Processed => 1,
            Self::Render => 2,
        }
    }
    fn mask(self) -> u32 {
        match self {
            Self::Raw => 0x0f,
            Self::Processed => 0x10,
            Self::Render => 0x20,
        }
    }
    fn frame_bytes(self) -> usize {
        if self == Self::Raw {
            16
        } else {
            2
        }
    }
    fn tick_step(self) -> i64 {
        match self {
            Self::Raw => 1,
            Self::Processed => 3,
            Self::Render => 2,
        }
    }
}

struct AudioBytes(Vec<u8>);
impl Drop for AudioBytes {
    fn drop(&mut self) {
        for byte in &mut self.0 {
            // Best-effort hygiene; not a secure-memory/allocator guarantee.
            unsafe { std::ptr::write_volatile(byte, 0) };
        }
        std::sync::atomic::compiler_fence(Ordering::SeqCst);
    }
}

struct Event {
    generation: u64,
    sequence: u64,
    source: Source,
    observed: Instant,
    bytes: AudioBytes,
}

#[derive(Default)]
struct Control {
    generation: AtomicU64,
    dropped_mask: AtomicU32,
    stop: AtomicBool,
    sequences: [AtomicU64; 3],
}

struct Tap {
    sender: SyncSender<Event>,
    control: Arc<Control>,
}

impl Tap {
    fn push(&self, source: Source, bytes: &[u8], observed: Instant) {
        let generation = self.control.generation.load(Ordering::Acquire);
        if generation == 0 {
            return;
        } // No allocation while disabled.
        let sequence = self.control.sequences[source.index()].fetch_add(1, Ordering::Relaxed);
        if bytes.is_empty() || bytes.len() > MAX_EVENT_BYTES {
            self.control
                .dropped_mask
                .fetch_or(source.mask(), Ordering::Relaxed);
            return;
        }
        let event = Event {
            generation,
            sequence,
            source,
            observed,
            bytes: AudioBytes(bytes.to_vec()),
        };
        // Full/disconnected queues are observations of missing data, never waits.
        if self.sender.try_send(event).is_err() {
            self.control
                .dropped_mask
                .fetch_or(source.mask(), Ordering::Relaxed);
        }
    }
}

static TAP: OnceLock<Tap> = OnceLock::new();

pub fn tap_raw(bytes: &[u8]) {
    if let Some(tap) = TAP.get() {
        tap.push(Source::Raw, bytes, Instant::now());
    }
}
pub fn tap_processed(bytes: &[u8]) {
    if let Some(tap) = TAP.get() {
        tap.push(Source::Processed, bytes, Instant::now());
    }
}
pub fn tap_render_datagram(bytes: &[u8]) {
    if let Some(tap) = TAP.get() {
        tap.push(Source::Render, bytes, Instant::now());
    }
}

struct Collection {
    generation: u64,
    epoch: Instant,
    window: ProbeWindow,
    next_tick: [Option<i64>; 3],
    last_sequence: [Option<u64>; 3],
    last_render_sequence: Option<u64>,
    render_seen: bool,
    input_error_mask: u32,
    arrival_deviation_us: [u64; 3],
    events: u32,
}

impl Collection {
    fn new(generation: u64, epoch: Instant) -> Self {
        Self {
            generation,
            epoch,
            window: ProbeWindow::new(
                WINDOW_START,
                Timing {
                    basis: TimingBasis::MonotonicEstimate,
                    uncertainty_ticks_48k: None,
                },
            )
            .expect("constant aligned window"),
            next_tick: [None; 3],
            last_sequence: [None; 3],
            last_render_sequence: None,
            render_seen: false,
            input_error_mask: 0,
            arrival_deviation_us: [0; 3],
            events: 0,
        }
    }

    fn invalidate(&mut self, mask: u32) {
        self.input_error_mask |= mask;
        for lane in 0..6 {
            if mask & (1 << lane) != 0 {
                self.window.mark_gap(lane);
            }
        }
    }

    fn observe(&mut self, event: &Event) {
        if event.generation != self.generation || event.observed < self.epoch {
            return;
        }
        self.events = self.events.saturating_add(1);
        if self.events >= MAX_COLLECTION_EVENTS {
            self.invalidate(0x3f);
            return;
        }
        let source = event.source;
        if self.last_sequence[source.index()]
            .is_some_and(|last| last.checked_add(1) != Some(event.sequence))
        {
            self.invalidate(source.mask());
        }
        self.last_sequence[source.index()] = Some(event.sequence);
        let mut pcm = event.bytes.0.as_slice();
        if source == Source::Render {
            if pcm == RENDER_START || pcm == RENDER_STOP {
                // A transition during observation prevents interpreting one
                // continuous sample timeline across different play sessions.
                if self.render_seen {
                    self.invalidate(source.mask());
                }
                self.last_render_sequence = None;
                self.next_tick[source.index()] = None;
                return;
            }
            let header = RENDER_MAGIC.len() + 12;
            if pcm.len() < header || !pcm.starts_with(RENDER_MAGIC) {
                self.invalidate(source.mask());
                return;
            }
            let seq = u64::from_le_bytes(
                pcm[RENDER_MAGIC.len()..RENDER_MAGIC.len() + 8]
                    .try_into()
                    .unwrap(),
            );
            let len = u32::from_le_bytes(pcm[RENDER_MAGIC.len() + 8..header].try_into().unwrap())
                as usize;
            if seq == 0
                || len == 0
                || len > MAX_PCM_BYTES
                || len % 2 != 0
                || pcm.len() != header + len
                || self
                    .last_render_sequence
                    .is_some_and(|last| last.checked_add(1) != Some(seq))
            {
                self.invalidate(source.mask());
                return;
            }
            // A measurement may start in the middle of an existing playback.
            self.last_render_sequence = Some(seq);
            self.render_seen = true;
            pcm = &pcm[header..];
        }
        if pcm.is_empty() || pcm.len() > MAX_PCM_BYTES || pcm.len() % source.frame_bytes() != 0 {
            self.invalidate(source.mask());
            return;
        }
        let duration = (pcm.len() / source.frame_bytes()) as i64 * source.tick_step();
        let observed_end = (event.observed.duration_since(self.epoch).as_micros() * 48 / 1000)
            .min(i64::MAX as u128) as i64;
        // Only the first block anchors each stream. Later blocks advance by
        // sample counts, not scheduler jitter. Initial queue age is UNKNOWN.
        let tick = self.next_tick[source.index()].unwrap_or_else(|| {
            let start = observed_end.saturating_sub(duration).max(0);
            start - start % source.tick_step()
        });
        let end = tick.saturating_add(duration);
        self.next_tick[source.index()] = Some(end);
        let deviation = observed_end.abs_diff(end).saturating_mul(1000) / 48;
        self.arrival_deviation_us[source.index()] =
            self.arrival_deviation_us[source.index()].max(deviation);
        let result = match source {
            Source::Raw => self.window.push_raw(tick, pcm),
            Source::Processed => self.window.push_processed(tick, pcm),
            Source::Render => self.window.push_render(tick, pcm),
        };
        if result.is_err() {
            self.invalidate(source.mask());
        }
    }

    fn finish(mut self, dropped_mask: u32) -> String {
        self.invalidate(dropped_mask);
        scalar_json(
            &self.window.report(),
            self.input_error_mask,
            self.arrival_deviation_us,
        )
    }
}

fn optional_number(number: Option<f64>) -> String {
    match number {
        Some(n) if n.is_finite() => format!("{n:.9}"),
        _ => "null".to_string(),
    }
}
fn channel_json(channel: ChannelReport) -> String {
    let l = channel.levels;
    let reason = channel
        .unavailable
        .map(|r| format!("\"{r:?}\""))
        .unwrap_or_else(|| "null".into());
    format!("{{\"samples\":{},\"zero\":{},\"nonzero\":{},\"container_extrema\":{},\"sum_squares\":{},\"rms\":{},\"complete_bins\":{},\"unavailable\":{}}}",
        l.sample_count, l.zero_count, l.nonzero_count, l.clipped_count,
        optional_number(Some(l.sum_squares)), optional_number(l.rms), channel.complete_bins, reason)
}
fn correlation_json(correlation: Result<Correlation, Unavailable>) -> String {
    match correlation {
        Ok(c) => format!("{{\"candidate_lag_us\":{},\"coefficient\":{},\"pairs\":{},\"unavailable\":null}}", c.lag_us, optional_number(Some(c.coefficient)), c.compared_samples),
        Err(r) => format!("{{\"candidate_lag_us\":null,\"coefficient\":null,\"pairs\":0,\"unavailable\":\"{r:?}\"}}"),
    }
}
fn scalar_json(report: &ProbeReport, error_mask: u32, arrival_deviation_us: [u64; 3]) -> String {
    // No caller strings or byte buffers enter this whitelist serializer.
    let raw = report
        .raw
        .iter()
        .map(|x| channel_json(*x))
        .collect::<Vec<_>>()
        .join(",");
    let correlations = report
        .raw_vs_render
        .iter()
        .map(|x| correlation_json(*x))
        .collect::<Vec<_>>()
        .join(",");
    format!(concat!("{{\"schema\":\"FVAP1\",\"acceptance\":\"not_evaluated\",\"full_duplex_capable\":false,",
        "\"window_ms\":1024,\"lag_step_us\":500,\"search_limit_us\":256000,",
        "\"timing_basis\":\"monotonic_first_block_then_sample_count\",\"timing_uncertainty_us\":null,",
        "\"acoustic_delay_us\":null,\"clock_drift_ppm\":null,\"erle_db\":null,",
        "\"software_reference_tap\":\"post_aplay_pipe_write_pre_alsa_mix\",",
        "\"filter\":\"2khz_boxcar_diagnostic_not_fullband\",\"input_error_mask\":{},",
        "\"max_arrival_sample_deviation_us\":[{},{},{}],\"raw\":[{}],",
        "\"processed\":{},\"software_render\":{},\"raw_vs_render\":[{}],\"processed_vs_render\":{}}}\n"),
        error_mask, arrival_deviation_us[0], arrival_deviation_us[1], arrival_deviation_us[2], raw,
        channel_json(report.processed), channel_json(report.software_render), correlations,
        correlation_json(report.processed_vs_render))
}

#[cfg(unix)]
mod local {
    use super::*;
    use std::ffi::{c_int, c_void};
    use std::fs::{self, Permissions};
    use std::io::{self, ErrorKind, Read, Write};
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::PathBuf;
    use std::thread::{self, JoinHandle};

    // Linux ARM uses asm-generic/socket.h: SOL_SOCKET=1, SO_PEERCRED=17.
    // The Linux ucred/socklen_t ABI fields below are 32 bits on ARM and x86_64.
    // Primary sources are recorded in ACOUSTIC-PROBE.md.
    #[repr(C)]
    struct Ucred {
        pid: u32,
        uid: u32,
        gid: u32,
    }
    unsafe extern "C" {
        fn getsockopt(
            fd: c_int,
            level: c_int,
            option: c_int,
            value: *mut c_void,
            len: *mut u32,
        ) -> c_int;
        fn geteuid() -> u32;
    }

    fn root_peer_pid(stream: &UnixStream) -> Option<u32> {
        let mut value = Ucred {
            pid: 0,
            uid: u32::MAX,
            gid: u32::MAX,
        };
        let mut len = std::mem::size_of::<Ucred>() as u32;
        let result = unsafe {
            getsockopt(
                stream.as_raw_fd(),
                1,
                17,
                (&mut value as *mut Ucred).cast(),
                &mut len,
            )
        };
        (result == 0 && len == std::mem::size_of::<Ucred>() as u32 && value.uid == 0)
            .then_some(value.pid)
    }

    pub fn maybe_run_client() -> Option<Result<(), String>> {
        let args = std::env::args().skip(1).collect::<Vec<_>>();
        if !args
            .first()
            .is_some_and(|a| a.starts_with("--acoustic-probe-"))
        {
            return None;
        }
        Some(run_client(&args).map_err(|e| format!("acoustic probe client failed: {e}")))
    }

    fn run_client(args: &[String]) -> io::Result<()> {
        if args.len() != 2 || unsafe { geteuid() } != 0 {
            return Err(io::Error::other("root and one owner PID required"));
        }
        let command = match args[0].as_str() {
            "--acoustic-probe-status" => REQUEST_STATUS,
            "--acoustic-probe-measure" => REQUEST_MEASURE,
            "--acoustic-probe-disable" => REQUEST_DISABLE,
            "--acoustic-probe-target" => target::REQUEST,
            _ => return Err(io::Error::other("unknown probe client command")),
        };
        if args[1].is_empty() || !args[1].bytes().all(|b| b.is_ascii_digit()) {
            return Err(io::Error::other("invalid owner PID"));
        }
        let pid = args[1]
            .parse::<u32>()
            .map_err(|_| io::Error::other("invalid owner PID"))?;
        if pid == 0 || pid > i32::MAX as u32 {
            return Err(io::Error::other("invalid owner PID"));
        }
        let path = PathBuf::from(format!("/tmp/family-voice-vpm-probe-{pid}.sock"));
        let meta = fs::symlink_metadata(&path)?;
        if !meta.file_type().is_socket()
            || meta.uid() != 0
            || meta.permissions().mode() & 0o777 != 0o600
        {
            return Err(io::Error::other("unsafe probe socket"));
        }
        let mut stream = UnixStream::connect(path)?;
        if root_peer_pid(&stream) != Some(pid) {
            return Err(io::Error::other("probe owner credential mismatch"));
        }
        stream.set_read_timeout(Some(Duration::from_secs(if command == target::REQUEST { 30 } else { 5 })))?;
        stream.set_write_timeout(Some(Duration::from_millis(100)))?;
        stream.write_all(command)?;
        stream.shutdown(std::net::Shutdown::Write)?;
        let mut response = Vec::new();
        Read::by_ref(&mut stream)
            .take(8193)
            .read_to_end(&mut response)?;
        if response.len() > 8192
            || !(response.starts_with(b"{\"schema\":\"FVAP1\"") || (command == target::REQUEST && response.starts_with(b"{\"schema\":\"FVAP2\"")))
            || !response.ends_with(b"\n")
            || !response.is_ascii()
            || response[..response.len() - 1].contains(&b'\n')
        {
            return Err(io::Error::other("invalid bounded scalar response"));
        }
        io::stdout().write_all(&response)
    }

    struct SocketGuard {
        path: PathBuf,
        dev: u64,
        ino: u64,
    }
    impl Drop for SocketGuard {
        fn drop(&mut self) {
            if let Ok(m) = fs::symlink_metadata(&self.path) {
                if m.file_type().is_socket()
                    && m.dev() == self.dev
                    && m.ino() == self.ino
                    && m.uid() == 0
                {
                    let _ = fs::remove_file(&self.path);
                }
            }
        }
    }

    pub struct Runtime {
        control: Arc<Control>,
        thread: Option<JoinHandle<()>>,
    }
    impl Runtime {
        pub fn start() -> io::Result<Self> {
            if unsafe { geteuid() } != 0 {
                return Err(io::Error::other("root-only probe"));
            }
            // PID-scoped, create-only: never unlink an existing path to bind.
            let path = PathBuf::from(format!(
                "/tmp/family-voice-vpm-probe-{}.sock",
                std::process::id()
            ));
            let listener = UnixListener::bind(&path)?;
            let m = fs::symlink_metadata(&path)?;
            if !m.file_type().is_socket() || m.uid() != 0 {
                return Err(io::Error::other("unsafe probe socket"));
            }
            let guard = SocketGuard {
                path: path.clone(),
                dev: m.dev(),
                ino: m.ino(),
            };
            fs::set_permissions(&path, Permissions::from_mode(0o600))?;
            listener.set_nonblocking(true)?;
            let control = Arc::new(Control::default());
            let (sender, receiver) = mpsc::sync_channel(QUEUE_CAPACITY);
            TAP.set(Tap {
                sender,
                control: control.clone(),
            })
            .map_err(|_| io::Error::other("probe already initialized"))?;
            let worker_control = control.clone();
            let worker = thread::Builder::new()
                .name("fv-acoustic-probe".into())
                .spawn(move || {
                    let _guard = guard;
                    worker_loop(listener, receiver, worker_control);
                })?;
            Ok(Self {
                control,
                thread: Some(worker),
            })
        }
    }
    impl Drop for Runtime {
        fn drop(&mut self) {
            self.control.generation.store(0, Ordering::Release);
            self.control.stop.store(true, Ordering::Release);
            if let Some(worker) = self.thread.take() {
                let _ = worker.join();
            }
        }
    }

    fn worker_loop(listener: UnixListener, receiver: Receiver<Event>, control: Arc<Control>) {
        let mut generation = 0u64;
        let mut next_measure = Instant::now();
        while !control.stop.load(Ordering::Acquire) {
            let mut stream = match listener.accept() {
                Ok((stream, _)) => stream,
                Err(e) if e.kind() == ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10));
                    continue;
                }
                Err(_) => break,
            };
            if root_peer_pid(&stream).is_none() {
                continue;
            }
            if stream
                .set_read_timeout(Some(Duration::from_millis(100)))
                .is_err()
                || stream
                    .set_write_timeout(Some(Duration::from_millis(100)))
                    .is_err()
            {
                continue;
            }
            // One bounded newline-terminated command per connection, no payload.
            let mut request = [0u8; 32];
            let mut length = 0;
            let request_started = Instant::now();
            while length < request.len() && request_started.elapsed() < Duration::from_millis(100) {
                match stream.read(&mut request[length..length + 1]) {
                    Ok(1) => {
                        length += 1;
                        if request[length - 1] == b'\n' {
                            break;
                        }
                    }
                    _ => break,
                }
            }
            if &request[..length] == REQUEST_STATUS {
                let _ = stream.write_all(
                    b"{\"schema\":\"FVAP1\",\"state\":\"idle\",\"raw_audio_output\":false}\n",
                );
            } else if &request[..length] == REQUEST_DISABLE {
                control.generation.store(0, Ordering::Release);
                control.stop.store(true, Ordering::Release);
                let _ = stream.write_all(b"{\"schema\":\"FVAP1\",\"state\":\"disabled\"}\n");
            } else if &request[..length] == target::REQUEST && Instant::now() >= next_measure {
                while receiver.try_recv().is_ok() {}
                control.dropped_mask.store(0, Ordering::Release);
                generation = generation.wrapping_add(1).max(1);
                let epoch = Instant::now();
                let mut collection = target::Collection::new(generation, epoch);
                control.generation.store(generation, Ordering::Release);
                while epoch.elapsed() < target::LIMIT && collection.events < target::MAX_EVENTS
                    && !control.stop.load(Ordering::Acquire) {
                    if let Ok(event) = receiver.recv_timeout(Duration::from_millis(10)) {
                        collection.observe(&event);
                    }
                }
                control.generation.store(0, Ordering::Release);
                for _ in 0..QUEUE_CAPACITY {
                    if let Ok(event) = receiver.try_recv() { collection.observe(&event); } else { break; }
                }
                let json = collection.finish(control.dropped_mask.load(Ordering::Acquire));
                debug_assert!(json.len() < 8192);
                let _ = stream.write_all(json.as_bytes());
                next_measure = Instant::now() + Duration::from_secs(3);
            } else if &request[..length] == REQUEST_MEASURE && Instant::now() >= next_measure {
                while receiver.try_recv().is_ok() {}
                control.dropped_mask.store(0, Ordering::Release);
                generation = generation.wrapping_add(1).max(1);
                let epoch = Instant::now();
                let mut collection = Collection::new(generation, epoch);
                control.generation.store(generation, Ordering::Release);
                while Instant::now().duration_since(epoch) < COLLECTION_LIMIT
                    && collection.events < MAX_COLLECTION_EVENTS
                    && !control.stop.load(Ordering::Acquire)
                {
                    if let Ok(event) = receiver.recv_timeout(Duration::from_millis(10)) {
                        collection.observe(&event);
                    }
                }
                control.generation.store(0, Ordering::Release);
                // Process the bounded tail only; no producer can extend the window.
                for _ in 0..QUEUE_CAPACITY {
                    if let Ok(event) = receiver.try_recv() {
                        collection.observe(&event);
                    } else {
                        break;
                    }
                }
                let mask = control.dropped_mask.load(Ordering::Acquire);
                let json = collection.finish(mask);
                debug_assert!(json.len() < 8192);
                let _ = stream.write_all(json.as_bytes());
                next_measure = Instant::now() + Duration::from_secs(3);
            } else {
                let _ = stream
                    .write_all(b"{\"schema\":\"FVAP1\",\"state\":\"rejected_or_cooldown\"}\n");
            }
            request.fill(0);
        }
        control.generation.store(0, Ordering::Release);
        while receiver.try_recv().is_ok() {}
    }

    #[cfg(test)]
    mod socket_tests {
        use super::*;

        // Standalone test binary only: no VPM symbols, microphone or playback.
        #[test]
        fn root_private_socket_status_reject_disable_and_cleanup() {
            assert_eq!(
                unsafe { geteuid() },
                0,
                "run isolated Linux socket fixture as root"
            );
            let runtime = Runtime::start().expect("private fixture listener");
            let path = PathBuf::from(format!(
                "/tmp/family-voice-vpm-probe-{}.sock",
                std::process::id()
            ));
            assert_eq!(
                fs::symlink_metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            let exchange = |request: &[u8]| {
                let mut stream = UnixStream::connect(&path).unwrap();
                assert_eq!(root_peer_pid(&stream), Some(std::process::id()));
                stream
                    .set_read_timeout(Some(Duration::from_secs(30)))
                    .unwrap();
                stream.write_all(request).unwrap();
                stream.shutdown(std::net::Shutdown::Write).unwrap();
                let mut response = String::new();
                stream.read_to_string(&mut response).unwrap();
                response
            };
            assert!(exchange(REQUEST_STATUS).contains("\"state\":\"idle\""));
            assert!(exchange(b"FVAP1 RAW\n").contains("rejected_or_cooldown"));
            assert_eq!(runtime.control.generation.load(Ordering::Acquire), 0);
            let target = exchange(target::REQUEST);
            assert!(target.starts_with("{\"schema\":\"FVAP2\""));
            assert_eq!(target.matches("\"identity_usable\":false").count(), 5);
            assert!(target.len() < 8192);
            assert_eq!(runtime.control.generation.load(Ordering::Acquire), 0);
            assert!(exchange(target::REQUEST).contains("rejected_or_cooldown"));
            assert!(exchange(REQUEST_DISABLE).contains("\"state\":\"disabled\""));
            drop(runtime);
            assert!(!path.exists());
        }
    }
}

#[cfg(unix)]
pub use local::{maybe_run_client, Runtime};

#[cfg(test)]
mod tests {
    use super::*;

    fn event(
        generation: u64,
        source: Source,
        bytes: Vec<u8>,
        epoch: Instant,
        end_ms: u64,
    ) -> Event {
        Event {
            generation,
            sequence: 1,
            source,
            observed: epoch + Duration::from_millis(end_ms),
            bytes: AudioBytes(bytes),
        }
    }
    fn render(sequence: u64, bytes: &[u8]) -> Vec<u8> {
        let mut result = RENDER_MAGIC.to_vec();
        result.extend_from_slice(&sequence.to_le_bytes());
        result.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        result.extend_from_slice(bytes);
        result
    }

    #[test]
    fn disabled_tap_never_enqueues_or_changes_audio() {
        let (sender, receiver) = mpsc::sync_channel(1);
        let control = Arc::new(Control::default());
        let tap = Tap {
            sender,
            control: control.clone(),
        };
        let pcm = [17u8; 16];
        tap.push(Source::Raw, &pcm, Instant::now());
        assert!(receiver.try_recv().is_err());
        assert_eq!(pcm, [17; 16]);
        assert_eq!(control.dropped_mask.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn full_or_disconnected_queue_fails_measurement_without_waiting() {
        let (sender, receiver) = mpsc::sync_channel(1);
        let control = Arc::new(Control::default());
        control.generation.store(2, Ordering::Relaxed);
        let tap = Tap {
            sender,
            control: control.clone(),
        };
        tap.push(Source::Raw, &[0; 16], Instant::now());
        tap.push(Source::Raw, &[0; 16], Instant::now());
        assert_eq!(control.dropped_mask.load(Ordering::Relaxed), 0x0f);
        drop(receiver);
        tap.push(Source::Processed, &[0; 2], Instant::now());
        assert_eq!(control.dropped_mask.load(Ordering::Relaxed), 0x1f);
    }

    #[test]
    fn oversized_event_is_rejected_before_copy() {
        let (sender, receiver) = mpsc::sync_channel(1);
        let control = Arc::new(Control::default());
        control.generation.store(1, Ordering::Relaxed);
        let tap = Tap {
            sender,
            control: control.clone(),
        };
        tap.push(
            Source::Render,
            &vec![3; MAX_EVENT_BYTES + 1],
            Instant::now(),
        );
        assert!(receiver.try_recv().is_err());
        assert_eq!(control.dropped_mask.load(Ordering::Relaxed), 0x20);
    }

    #[test]
    fn stale_generations_and_pre_epoch_blocks_are_not_replayed() {
        let epoch = Instant::now();
        let mut c = Collection::new(2, epoch);
        c.observe(&event(1, Source::Raw, vec![3; 16], epoch, 1));
        let old = Event {
            generation: 2,
            sequence: 1,
            source: Source::Raw,
            observed: epoch - Duration::from_millis(1),
            bytes: AudioBytes(vec![3; 16]),
        };
        c.observe(&old);
        assert_eq!(c.window.report().raw[0].levels.sample_count, 0);
        assert_eq!(c.next_tick, [None; 3]);
    }

    #[test]
    fn render_gap_and_transition_cannot_produce_valid_correlation() {
        let epoch = Instant::now();
        let mut c = Collection::new(1, epoch);
        c.observe(&event(1, Source::Render, render(9, &[1; 960]), epoch, 20));
        let mut second = event(1, Source::Render, render(11, &[1; 960]), epoch, 40);
        second.sequence = 2;
        c.observe(&second);
        assert_eq!(c.input_error_mask, 0x20);
        let json = c.finish(0);
        assert!(json.contains("Discontinuity"));
        let mut c = Collection::new(1, epoch);
        c.observe(&event(1, Source::Render, render(9, &[1; 960]), epoch, 20));
        let mut stop = event(1, Source::Render, RENDER_STOP.to_vec(), epoch, 21);
        stop.sequence = 2;
        c.observe(&stop);
        assert_eq!(c.input_error_mask, 0x20);
    }

    #[test]
    fn malformed_render_and_invalid_pcm_fail_closed() {
        let epoch = Instant::now();
        let mut c = Collection::new(1, epoch);
        c.observe(&event(1, Source::Render, b"not audio".to_vec(), epoch, 2));
        c.observe(&event(1, Source::Processed, vec![1; 3], epoch, 2));
        c.observe(&event(1, Source::Raw, vec![1; 15], epoch, 2));
        assert_eq!(c.input_error_mask, 0x3f);
    }

    #[test]
    fn source_sequence_catches_drops_even_without_a_drop_mask() {
        let epoch = Instant::now();
        let mut c = Collection::new(1, epoch);
        c.observe(&event(1, Source::Raw, vec![1; 16], epoch, 1));
        let mut later = event(1, Source::Raw, vec![1; 16], epoch, 2);
        later.sequence = 3;
        c.observe(&later);
        assert_eq!(c.input_error_mask, 0x0f);
        assert!(c.finish(0).contains("Discontinuity"));
    }

    #[test]
    fn event_budget_invalidates_an_overloaded_measurement() {
        let epoch = Instant::now();
        let mut c = Collection::new(1, epoch);
        for sequence in 0..u64::from(MAX_COLLECTION_EVENTS) {
            let mut e = event(1, Source::Raw, vec![0; 16], epoch, sequence);
            e.sequence = sequence;
            c.observe(&e);
        }
        assert_eq!(c.events, MAX_COLLECTION_EVENTS);
        assert_eq!(c.input_error_mask, 0x3f);
    }

    #[test]
    fn serialization_is_bounded_scalars_not_pcm_or_acceptance() {
        let epoch = Instant::now();
        let json = Collection::new(1, epoch).finish(0x0f);
        assert!(json.len() < 8192);
        assert!(json.contains("\"full_duplex_capable\":false"));
        assert!(json.contains("\"erle_db\":null"));
        assert!(json.contains("\"acoustic_delay_us\":null"));
        assert!(json.contains("\"timing_uncertainty_us\":null"));
        assert!(!json.contains("NaN"));
        assert!(!json.contains("Infinity"));
        assert!(!json.contains("base64"));
        assert!(!json.contains("\"pcm\""));
        assert_eq!(MAX_EVENT_BYTES * QUEUE_CAPACITY, 524_992);
    }

    #[test]
    fn real_tap_queue_collection_reports_known_lag_without_retiming_jitter() {
        let epoch = Instant::now();
        let (sender, receiver) = mpsc::sync_channel(QUEUE_CAPACITY);
        let control = Arc::new(Control::default());
        control.generation.store(7, Ordering::Relaxed);
        let tap = Tap {
            sender,
            control: control.clone(),
        };
        let mut c = Collection::new(7, epoch);
        fn noise(bin: i64) -> i16 {
            let mut x = (bin as u32)
                .wrapping_mul(747796405)
                .wrapping_add(2891336453);
            x = ((x >> ((x >> 28) + 4)) ^ x).wrapping_mul(277803737);
            (((x >> 22) ^ x) % 24001) as i16 - 12000
        }
        for block in 0..200 {
            let tick = block * 384;
            let mut raw = Vec::new();
            let mut processed = Vec::new();
            let mut software = Vec::new();
            for sample in 0..384 {
                let value = i32::from(noise((tick + sample) / 24 - 31)) * 32768;
                for channel in [value, -value, 0, value / 2] {
                    raw.extend_from_slice(&channel.to_le_bytes());
                }
            }
            for sample in 0..128 {
                processed
                    .extend_from_slice(&(noise((tick + sample * 3) / 24 - 31) / 2).to_le_bytes());
            }
            for sample in 0..192 {
                software.extend_from_slice(&noise((tick + sample * 2) / 24).to_le_bytes());
            }
            let jitter_ms = if block == 0 { 0 } else { (block % 3) as u64 };
            let observed = epoch + Duration::from_millis((block as u64 + 1) * 8 + jitter_ms);
            let before = raw.clone();
            tap.push(Source::Raw, &raw, observed);
            tap.push(Source::Processed, &processed, observed);
            tap.push(
                Source::Render,
                &render(block as u64 + 50, &software),
                observed,
            );
            assert_eq!(raw, before);
            while let Ok(e) = receiver.try_recv() {
                c.observe(&e);
            }
        }
        let report = c.window.report();
        assert_eq!(report.raw_vs_render[0].unwrap().lag_us, 15500);
        assert!(report.raw_vs_render[1].unwrap().coefficient < -0.99);
        assert_eq!(report.processed_vs_render.unwrap().lag_us, 15500);
        assert_eq!(report.timing.uncertainty_ticks_48k, None);
        assert!(c.arrival_deviation_us.iter().all(|n| *n >= 2000));
        assert_eq!(control.dropped_mask.load(Ordering::Relaxed), 0);
        let json = c.finish(0);
        assert!(json.contains("\"candidate_lag_us\":15500"));
        assert!(json.contains("\"acoustic_delay_us\":null"));
    }
}
