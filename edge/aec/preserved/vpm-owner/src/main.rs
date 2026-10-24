// SPDX-License-Identifier: LGPL-3.0-or-later
//
// Minimal OH2P VPM/FlexKWS owner. The VPM ABI layout is derived from
// stevenjoezhang/xiaoai-agent commit 9fc56179fbdde610e95755a843615669dece0d08.

use std::env;
use std::collections::VecDeque;
use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::fs::{File, OpenOptions, Permissions};
use std::io::{ErrorKind, Read, Seek, SeekFrom, Write};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::fs::{FileTypeExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{SocketAddr, UnixDatagram, UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

mod render_aec;
mod native_trial;
mod native_trial_protocol;
mod stream_health;
#[cfg(feature = "acoustic-probe")]
mod acoustic_probe;

#[cfg(all(feature = "acoustic-probe", any(feature = "speexdsp-static", feature = "speexdsp-dynamic-test")))]
compile_error!("acoustic-probe must not be combined with a Speex AEC feature");

const INIT_SIZE: usize = 0x1b0;
const AUDIO_SAMPLE_RATE_OFFSET: usize = 316;
const AUDIO_CHANNELS_OFFSET: usize = 320;
const AUDIO_TIME_MS_OFFSET: usize = 324;
const AUDIO_FORMAT_OFFSET: usize = 328;
const OUTPUT_SAMPLE_RATE_OFFSET: usize = 332;
const OUTPUT_CHANNELS_OFFSET: usize = 336;
const OUTPUT_FORMAT_OFFSET: usize = 344;
const REF_CH_INDEX_OFFSET: usize = 348;
const WAKEUP_PREFIX_MS_OFFSET: usize = 352;
const WAKEUP_SUFFIX_MS_OFFSET: usize = 356;
const WAIT_ASR_TIMEOUT_MS_OFFSET: usize = 360;
const VAD_TIMEOUT_MS_OFFSET: usize = 364;
const VAD_SWITCH_OFFSET: usize = 368;
const WAKEUP_DATA_SWITCH_OFFSET: usize = 372;
const WAKEUP_SWITCH_OFFSET: usize = 376;
const TARGET_SCORE_OFFSET: usize = 380;
const EFFECT_MODE_OFFSET: usize = 384;
const CONFIG_PATH_OFFSET: usize = 388;
const USER_CTX_OFFSET: usize = 392;
const LOG_CALLBACK_OFFSET: usize = 396;
const EVENT_CALLBACK_OFFSET: usize = 400;
const WAKEUP_DATA_CALLBACK_OFFSET: usize = 404;
const ASR_DATA_CALLBACK_OFFSET: usize = 408;
const VOIP_DATA_CALLBACK_OFFSET: usize = 412;
const STAT_CALLBACK_OFFSET: usize = 424;

const SAMPLE_RATE: u32 = 48_000;
const CHANNELS: u32 = 4;
const BITS_PER_SAMPLE: u32 = 32;
const FRAME_MS: u32 = 8;
const PERIOD_SIZE: u32 = 384;
const BUFFER_SIZE: u32 = 4096;
const FRAME_BYTES: usize = (SAMPLE_RATE as usize * FRAME_MS as usize / 1000)
    * CHANNELS as usize
    * (BITS_PER_SAMPLE as usize / 8);
const EVENT_DEBOUNCE: Duration = Duration::from_millis(1200);
const CAPTURE_SOCKET_PATH: &str = "/tmp/family-voice-vpm-capture.sock";
const PREROLL_SOCKET_PATH: &str = "/tmp/family-voice-vpm-preroll.sock";
const CAPTURE_WINDOW: Duration = Duration::from_secs(5);
const MAX_CAPTURE_BYTES: usize = 16_000 * 2 * 5;
const PREROLL_MAX_BYTES: usize = 16_000 * 2 * 2;
const PREROLL_TTL: Duration = Duration::from_secs(3);
const PREROLL_REQUEST: &[u8] = b"FVPR1 GET\n";
const PREROLL_RESPONSE_MAGIC: &[u8] = b"FVPR1 PCM\n";
const PREROLL_EMPTY_RESPONSE: &[u8] = b"FVPR1 NIL\n";
const SHERPA_WAKE_EVENT_PATH: &str = "/tmp/open-xiaoai/kws.log";
const SHERPA_WAKE_EVENT_SCAN_BYTES: u64 = 4_096;
// A single VPM owner fans out this bounded enhanced stream to local consumers.
// Datagram delivery is deliberately lossy/non-blocking: a slow consumer may
// miss a frame, but can never stall ALSA/VPM processing or grow owner memory.
const SHARED_AUDIO_SOCKET_PATH: &str = "/tmp/family-voice-vpm-audio.sock";
const SHARED_AUDIO_LIVE_PATH: &str = "/tmp/open-xiaoai/shared-audio-owner-live";
const SHARED_AUDIO_SUBSCRIBE: &[u8] = b"FVSH1 SUB\n";
const SHARED_AUDIO_UNSUBSCRIBE: &[u8] = b"FVSH1 UNSUB\n";
const SHARED_AUDIO_ACK: &[u8] = b"FVSH1 OK\n";
const SHARED_AUDIO_MAGIC: &[u8] = b"FVSH1 PCM\n";
const SHARED_AUDIO_MAX_BYTES: usize = 16_384;
const SHARED_AUDIO_MAX_PEERS: usize = 4;
const SHARED_AUDIO_QUEUE_CAPACITY: usize = 16;

const O_RDONLY: c_int = 0;
const O_WRONLY: c_int = 1;
const O_CREAT: c_int = 0x40;
const O_APPEND: c_int = 0x400;
const O_NONBLOCK: c_int = 0x800;
// Linux ARM uses the legacy architecture-specific directory flag values,
// unlike asm-generic/x86. Keep these aligned with arm-linux-gnu bits/fcntl.h.
const O_DIRECTORY: c_int = 0x4000;
const O_NOFOLLOW: c_int = 0x8000;
const O_CLOEXEC: c_int = 0x80000;
const F_GETFL: c_int = 3;
const F_SETFL: c_int = 4;
const LOCK_EX: c_int = 2;
const LOCK_NB: c_int = 4;
const RTLD_NOW: c_int = 2;
const PR_SET_PDEATHSIG: c_int = 1;
const SIGTERM: c_int = 15;

static STOP: AtomicBool = AtomicBool::new(false);
static WRITER_FAILED: AtomicBool = AtomicBool::new(false);
static CAPTURE_ACTIVE: AtomicBool = AtomicBool::new(false);
static VPM_WAKE_EVENTS_ENABLED: AtomicBool = AtomicBool::new(true);
static SHARED_AUDIO_LIVE: AtomicBool = AtomicBool::new(false);
static ASR_CALLBACK_COUNT: AtomicU64 = AtomicU64::new(0);
static ASR_CALLBACK_BYTES: AtomicU64 = AtomicU64::new(0);
static ASR_CALLBACK_TYPE_1: AtomicU64 = AtomicU64::new(0);
static ASR_CALLBACK_TYPE_2: AtomicU64 = AtomicU64::new(0);
static ASR_CALLBACK_TYPE_3: AtomicU64 = AtomicU64::new(0);
static ENHANCED_CALLBACK_COUNT: AtomicU64 = AtomicU64::new(0);
static WAKE_TX: OnceLock<SyncSender<WakeEvent>> = OnceLock::new();
static ASR_TX: OnceLock<SyncSender<Vec<u8>>> = OnceLock::new();
static AUDIO_RING: OnceLock<Mutex<VecDeque<u8>>> = OnceLock::new();
static PREROLL_PENDING: OnceLock<Mutex<Option<PrerollSnapshot>>> = OnceLock::new();
static SHARED_TX: OnceLock<SyncSender<Vec<u8>>> = OnceLock::new();
static SHERPA_LAST_PREROLL_EVENT_MS: AtomicU64 = AtomicU64::new(0);

unsafe extern "C" {
    fn open(path: *const c_char, flags: c_int, ...) -> c_int;
    fn openat(dirfd: c_int, path: *const c_char, flags: c_int, ...) -> c_int;
    fn close(fd: c_int) -> c_int;
    fn fchmod(fd: c_int, mode: u32) -> c_int;
    fn flock(fd: c_int, operation: c_int) -> c_int;
    fn fcntl(fd: c_int, command: c_int, ...) -> c_int;
    fn getppid() -> c_int;
    fn prctl(option: c_int, ...) -> c_int;
    fn _exit(status: c_int) -> !;
    fn signal(signum: c_int, handler: usize) -> usize;
    fn dlopen(filename: *const c_char, flags: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    fn dlerror() -> *const c_char;
    fn dlclose(handle: *mut c_void) -> c_int;
}

#[derive(Clone, Copy)]
enum WakeEvent {
    Started,
    Real,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WakeMode {
    Vpm,
    Sherpa,
}

impl WakeMode {
    fn initial_status(self) -> c_int {
        match self {
            WakeMode::Sherpa => 2,
            WakeMode::Vpm => 6,
        }
    }

    fn publishes_vpm_wake(self) -> bool {
        match self {
            WakeMode::Sherpa => false,
            WakeMode::Vpm => true,
        }
    }

    fn capture_end_status(self) -> c_int {
        match self {
            WakeMode::Sherpa => 2,
            WakeMode::Vpm => 3,
        }
    }
}

#[repr(C)]
struct VpmInput {
    size: u32,
    reserved_04: u32,
    start_time_ms: u64,
    data_frame_number: u64,
    frame_size: u32,
    data: *const u8,
}

#[repr(C)]
struct VpmCallbackData {
    data_type: u32,
    reserved_04: u32,
    size: u32,
    reserved_0c: u32,
    reserved_10: u32,
    reserved_14: u32,
    reserved_18: u32,
    reserved_1c: u32,
    reserved_20: u32,
    data: *const u8,
}

struct PrerollSnapshot {
    pcm: Vec<u8>,
    created_at: Instant,
}

type VpmInit = unsafe extern "C" fn(*const c_void) -> c_int;
type VpmSimple = unsafe extern "C" fn() -> c_int;
type VpmSetStatus = unsafe extern "C" fn(c_int) -> c_int;
type VpmProcess = unsafe extern "C" fn(*const VpmInput) -> c_int;
type VpmGetVersionInfo = unsafe extern "C" fn() -> *const c_char;

extern "C" fn signal_handler(_signal: c_int) {
    STOP.store(true, Ordering::SeqCst);
}

extern "C" fn vpm_log_callback(_level: c_int, _tag: *const c_char, _message: *const c_char) {}

extern "C" fn vpm_event_callback(_ctx: *mut c_void, event: c_int, _value: c_int) {
    if event != 0 {
        return;
    }
    publish_preroll();
    publish_vpm_wake_event();
}

extern "C" fn vpm_wakeup_data_callback(_ctx: *mut c_void, _data: *const c_void, local_word: c_int) {
    if !is_real_wakeup_data(local_word) {
        return;
    }
    publish_preroll();
    publish_vpm_wake_event();
}

fn publish_vpm_wake_event() {
    if !VPM_WAKE_EVENTS_ENABLED.load(Ordering::Relaxed) {
        return;
    }
    if let Some(sender) = WAKE_TX.get() {
        let _ = sender.try_send(WakeEvent::Real);
    }
}

fn is_real_wakeup_data(local_word: c_int) -> bool {
    local_word == 0
}

extern "C" fn vpm_data_callback(_ctx: *mut c_void, data: *const VpmCallbackData) {
    if data.is_null() {
        return;
    }
    let packet = unsafe { &*data };
    ASR_CALLBACK_COUNT.fetch_add(1, Ordering::Relaxed);
    ASR_CALLBACK_BYTES.fetch_add(u64::from(packet.size), Ordering::Relaxed);
    match packet.data_type {
        1 => {
            ASR_CALLBACK_TYPE_1.fetch_add(1, Ordering::Relaxed);
        }
        2 => {
            ASR_CALLBACK_TYPE_2.fetch_add(1, Ordering::Relaxed);
        }
        3 => {
            ASR_CALLBACK_TYPE_3.fetch_add(1, Ordering::Relaxed);
        }
        _ => {}
    };
    // Public VPM data type 2 is the enhanced 16 kHz mono S16_LE middle stream.
    if packet.data_type != 2 || packet.data.is_null() || packet.size == 0 {
        return;
    }
    let Ok(size) = usize::try_from(packet.size) else {
        return;
    };
    if size > 16_384 || size % 2 != 0 {
        return;
    }
    let audio = unsafe { std::slice::from_raw_parts(packet.data, size) }.to_vec();
    ENHANCED_CALLBACK_COUNT.fetch_add(1, Ordering::Relaxed);
    #[cfg(feature = "acoustic-probe")]
    acoustic_probe::tap_processed(&audio);
    append_audio_ring(&audio);
    if let Some(sender) = SHARED_TX.get() {
        // The owner retains a separate bounded copy for fan-out. try_send is
        // required here because callbacks run on the VPM/ALSA hot path.
        let _ = sender.try_send(audio.clone());
    }
    if !CAPTURE_ACTIVE.load(Ordering::Relaxed) {
        return;
    }
    if let Some(sender) = ASR_TX.get() {
        let _ = sender.try_send(audio);
    }
}

fn append_audio_ring(audio: &[u8]) {
    let ring = AUDIO_RING.get_or_init(|| Mutex::new(VecDeque::with_capacity(PREROLL_MAX_BYTES)));
    let Ok(mut ring) = ring.lock() else {
        return;
    };
    append_bounded_audio(&mut ring, audio);
}

fn append_bounded_audio(ring: &mut VecDeque<u8>, audio: &[u8]) {
    for byte in audio {
        if ring.len() == PREROLL_MAX_BYTES {
            let _ = ring.pop_front();
        }
        ring.push_back(*byte);
    }
}

fn publish_preroll() {
    let Some(ring) = AUDIO_RING.get() else {
        return;
    };
    let Ok(ring) = ring.lock() else {
        return;
    };
    if ring.is_empty() {
        return;
    }
    let snapshot = PrerollSnapshot {
        pcm: ring.iter().copied().collect(),
        created_at: Instant::now(),
    };
    let pending = PREROLL_PENDING.get_or_init(|| Mutex::new(None));
    let Ok(mut pending) = pending.lock() else {
        return;
    };
    if let Some(previous) = pending.replace(snapshot) {
        let mut previous = previous.pcm;
        previous.fill(0);
    }
}
extern "C" fn vpm_stat_callback(_a0: usize, _a1: usize, _a2: usize) {}

struct FdGuard(RawFd);

impl Drop for FdGuard {
    fn drop(&mut self) {
        if self.0 >= 0 {
            unsafe {
                close(self.0);
            }
        }
    }
}

struct Library(*mut c_void);

impl Library {
    fn load(path: &CStr) -> Result<Self, String> {
        let handle = unsafe { dlopen(path.as_ptr(), RTLD_NOW) };
        if handle.is_null() {
            return Err(format!("dlopen failed: {}", dl_error()));
        }
        Ok(Self(handle))
    }

    unsafe fn symbol<T: Copy>(&self, name: &CStr) -> Result<T, String> {
        let _ = unsafe { dlerror() };
        let pointer = unsafe { dlsym(self.0, name.as_ptr()) };
        if pointer.is_null() {
            return Err(format!(
                "dlsym {} failed: {}",
                name.to_string_lossy(),
                dl_error()
            ));
        }
        if std::mem::size_of::<T>() != std::mem::size_of::<*mut c_void>() {
            return Err("function pointer size mismatch".to_string());
        }
        Ok(unsafe { std::mem::transmute_copy(&pointer) })
    }
}

impl Drop for Library {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                dlclose(self.0);
            }
        }
    }
}

struct VpmRuntime {
    stop: VpmSimple,
    release: VpmSimple,
    set_status: VpmSetStatus,
    initialized: bool,
    started: bool,
}

impl Drop for VpmRuntime {
    fn drop(&mut self) {
        unsafe {
            if self.started {
                let _ = (self.set_status)(6);
                let _ = (self.stop)();
            }
            if self.initialized {
                let _ = (self.release)();
            }
        }
    }
}

struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn main() {
    #[cfg(feature = "acoustic-probe")]
    if let Some(result) = acoustic_probe::maybe_run_client() {
        if let Err(error) = result {
            eprintln!("vpm-owner: {error}");
            std::process::exit(1);
        }
        return; // Client mode never initializes VPM, locks, or arecord.
    }
    if let Err(error) = run() {
        eprintln!("vpm-owner: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    if std::mem::size_of::<usize>() != 4 {
        return Err("vpm-owner requires a 32-bit ARM build".to_string());
    }
    let options = parse_options()?;
    VPM_WAKE_EVENTS_ENABLED.store(options.wake_mode.publishes_vpm_wake(), Ordering::Relaxed);
    install_signal_handlers()?;

    let event_file = open_event_file()?;
    let (sender, receiver) = mpsc::sync_channel(16);
    WAKE_TX
        .set(sender.clone())
        .map_err(|_| "wake sender was already initialized".to_string())?;
    let writer = thread::spawn(move || writer_loop(event_file, receiver));

    let result = run_vpm(&options, sender);
    STOP.store(true, Ordering::SeqCst);
    if writer.join().is_err() {
        return Err("event writer thread panicked".to_string());
    }
    if WRITER_FAILED.load(Ordering::SeqCst) {
        return Err("event writer failed".to_string());
    }
    result
}

struct Options {
    no_audio: bool,
    seconds: u64,
    wake_mode: WakeMode,
}

fn parse_options() -> Result<Options, String> {
    let mut no_audio = false;
    let mut seconds = 0u64;
    let mut wake_mode = WakeMode::Vpm;
    let mut args = env::args().skip(1);
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--no-audio" => no_audio = true,
            "--seconds" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--seconds requires a value".to_string())?;
                seconds = value
                    .parse()
                    .map_err(|_| "--seconds must be an integer".to_string())?;
            }
            "--wake-mode" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--wake-mode requires vpm or sherpa".to_string())?;
                wake_mode = match value.as_str() {
                    "vpm" => WakeMode::Vpm,
                    "sherpa" => WakeMode::Sherpa,
                    _ => return Err("--wake-mode requires vpm or sherpa".to_string()),
                };
            }
            "--help" => {
                println!("vpm-owner [--no-audio] [--seconds N] [--wake-mode vpm|sherpa]");
                std::process::exit(0);
            }
            _ => return Err(format!("unknown argument: {argument}")),
        }
    }
    Ok(Options {
        no_audio,
        seconds,
        wake_mode,
    })
}

fn install_signal_handlers() -> Result<(), String> {
    for signal_number in [1, 2, 3, 15] {
        let previous = unsafe { signal(signal_number, signal_handler as *const () as usize) };
        if previous == usize::MAX {
            return Err(format!(
                "install signal handler {signal_number} failed: {}",
                std::io::Error::last_os_error()
            ));
        }
    }
    Ok(())
}

fn open_event_file() -> Result<File, String> {
    let tmp_name = c"/tmp";
    let directory_name = c"open-xiaoai";
    let file_name = c"kws.log";

    let tmp_fd = unsafe {
        open(
            tmp_name.as_ptr(),
            O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC,
        )
    };
    if tmp_fd < 0 {
        return Err(format!(
            "open /tmp failed: {}",
            std::io::Error::last_os_error()
        ));
    }
    let tmp_guard = FdGuard(tmp_fd);

    match std::fs::create_dir("/tmp/open-xiaoai") {
        Ok(()) => {}
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
        Err(error) => return Err(format!("create event directory failed: {error}")),
    }

    let directory_fd = unsafe {
        openat(
            tmp_guard.0,
            directory_name.as_ptr(),
            O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC,
        )
    };
    if directory_fd < 0 {
        return Err(format!(
            "open event directory failed: {}",
            std::io::Error::last_os_error()
        ));
    }
    let directory_guard = FdGuard(directory_fd);
    if unsafe { fchmod(directory_guard.0, 0o700) } != 0 {
        return Err(format!(
            "event directory chmod failed: {}",
            std::io::Error::last_os_error()
        ));
    }

    let file_fd = unsafe {
        openat(
            directory_guard.0,
            file_name.as_ptr(),
            O_WRONLY | O_CREAT | O_APPEND | O_NOFOLLOW | O_CLOEXEC,
            0o600,
        )
    };
    if file_fd < 0 {
        return Err(format!(
            "open event file failed: {}",
            std::io::Error::last_os_error()
        ));
    }
    let file = unsafe { File::from_raw_fd(file_fd) };
    let metadata = file
        .metadata()
        .map_err(|error| format!("event file metadata failed: {error}"))?;
    if !metadata.file_type().is_file() {
        return Err("event path is not a regular file".to_string());
    }
    file.set_permissions(Permissions::from_mode(0o600))
        .map_err(|error| format!("event file chmod failed: {error}"))?;
    let lock_result = unsafe { flock(file.as_raw_fd(), LOCK_EX | LOCK_NB) };
    if lock_result != 0 {
        return Err(format!(
            "another vpm-owner holds the event lock: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(file)
}

fn writer_loop(mut file: File, receiver: Receiver<WakeEvent>) {
    let mut last_wake: Option<Instant> = None;
    loop {
        if STOP.load(Ordering::SeqCst) {
            break;
        }
        match receiver.recv_timeout(Duration::from_millis(100)) {
            Ok(WakeEvent::Started) => {
                if write_event_line(&mut file, "__STARTED__").is_err() {
                    WRITER_FAILED.store(true, Ordering::SeqCst);
                    STOP.store(true, Ordering::SeqCst);
                    break;
                }
            }
            Ok(WakeEvent::Real) => {
                if last_wake
                    .map(|last| last.elapsed() < EVENT_DEBOUNCE)
                    .unwrap_or(false)
                {
                    continue;
                }
                last_wake = Some(Instant::now());
                if write_event_line(&mut file, "XIAOAITONGXUE").is_err() {
                    WRITER_FAILED.store(true, Ordering::SeqCst);
                    STOP.store(true, Ordering::SeqCst);
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
}

fn write_event_line(file: &mut File, keyword: &str) -> std::io::Result<()> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    writeln!(file, "{millis}@{keyword}")?;
    file.flush()
}

fn run_vpm(options: &Options, sender: SyncSender<WakeEvent>) -> Result<(), String> {
    let library_path = CString::new("/usr/lib/libvpm.so").unwrap();
    let config_path = CString::new("/usr/share/mipns/vpm/json_segment").unwrap();
    let library = Library::load(&library_path)?;

    let vpm_init: VpmInit = unsafe { library.symbol(c"vpm_init")? };
    let vpm_start: VpmSimple = unsafe { library.symbol(c"vpm_start")? };
    let vpm_stop: VpmSimple = unsafe { library.symbol(c"vpm_stop")? };
    let vpm_release: VpmSimple = unsafe { library.symbol(c"vpm_release")? };
    let vpm_set_status: VpmSetStatus = unsafe { library.symbol(c"vpm_set_status")? };
    let vpm_process: VpmProcess = unsafe { library.symbol(c"vpm_process")? };
    let vpm_get_version_info: VpmGetVersionInfo =
        unsafe { library.symbol(c"vpm_get_version_info")? };

    let mut runtime = VpmRuntime {
        stop: vpm_stop,
        release: vpm_release,
        set_status: vpm_set_status,
        initialized: false,
        started: false,
    };

    let version_pointer = unsafe { vpm_get_version_info() };
    if !version_pointer.is_null() {
        let version = unsafe { CStr::from_ptr(version_pointer) }.to_string_lossy();
        eprintln!("vpm-owner: libvpm {version}");
    }

    let mut init = vec![0u8; INIT_SIZE];
    write_ptr(&mut init, CONFIG_PATH_OFFSET, config_path.as_ptr() as usize)?;
    write_ptr(
        &mut init,
        USER_CTX_OFFSET,
        u32::from_be_bytes(*b"VPMP") as usize,
    )?;
    write_ptr(
        &mut init,
        LOG_CALLBACK_OFFSET,
        vpm_log_callback as *const () as usize,
    )?;
    write_ptr(
        &mut init,
        EVENT_CALLBACK_OFFSET,
        vpm_event_callback as *const () as usize,
    )?;
    write_ptr(
        &mut init,
        WAKEUP_DATA_CALLBACK_OFFSET,
        vpm_wakeup_data_callback as *const () as usize,
    )?;
    write_ptr(
        &mut init,
        ASR_DATA_CALLBACK_OFFSET,
        vpm_data_callback as *const () as usize,
    )?;
    write_ptr(
        &mut init,
        VOIP_DATA_CALLBACK_OFFSET,
        vpm_data_callback as *const () as usize,
    )?;
    write_ptr(
        &mut init,
        STAT_CALLBACK_OFFSET,
        vpm_stat_callback as *const () as usize,
    )?;
    write_u32(&mut init, AUDIO_SAMPLE_RATE_OFFSET, SAMPLE_RATE)?;
    write_u32(&mut init, AUDIO_CHANNELS_OFFSET, CHANNELS)?;
    write_u32(&mut init, AUDIO_TIME_MS_OFFSET, FRAME_MS)?;
    write_u32(&mut init, AUDIO_FORMAT_OFFSET, 3)?;
    write_u32(&mut init, OUTPUT_SAMPLE_RATE_OFFSET, 16_000)?;
    write_u32(&mut init, OUTPUT_CHANNELS_OFFSET, 1)?;
    write_u32(&mut init, OUTPUT_FORMAT_OFFSET, 0)?;
    write_u32(&mut init, REF_CH_INDEX_OFFSET, 3)?;
    write_u32(&mut init, WAKEUP_PREFIX_MS_OFFSET, 600)?;
    write_u32(&mut init, WAKEUP_SUFFIX_MS_OFFSET, 600)?;
    write_u32(&mut init, WAIT_ASR_TIMEOUT_MS_OFFSET, 6000)?;
    write_u32(&mut init, VAD_TIMEOUT_MS_OFFSET, 400)?;
    write_u32(&mut init, VAD_SWITCH_OFFSET, 1)?;
    write_u32(&mut init, WAKEUP_DATA_SWITCH_OFFSET, 1)?;
    write_u32(&mut init, WAKEUP_SWITCH_OFFSET, 1)?;
    write_f32(&mut init, TARGET_SCORE_OFFSET, 0.3)?;
    write_u32(&mut init, EFFECT_MODE_OFFSET, 1)?;

    let init_result = unsafe { vpm_init(init.as_ptr() as *const c_void) };
    if init_result != 0 {
        return Err(format!("vpm_init returned {init_result}"));
    }
    runtime.initialized = true;
    let start_result = unsafe { vpm_start() };
    if start_result != 0 {
        return Err(format!("vpm_start returned {start_result}"));
    }
    runtime.started = true;
    let initial_status = options.wake_mode.initial_status();
    let status_result = unsafe { vpm_set_status(initial_status) };
    if status_result != 0 {
        return Err(format!(
            "vpm_set_status({initial_status}) returned {status_result}"
        ));
    }
    if options.wake_mode.publishes_vpm_wake() {
        sender
            .try_send(WakeEvent::Started)
            .map_err(|error| format!("event writer is unavailable: {error}"))?;
    }

    let deadline = if options.seconds == 0 {
        None
    } else {
        Some(Instant::now() + Duration::from_secs(options.seconds))
    };
    if options.no_audio {
        while !should_stop(deadline) {
            thread::sleep(Duration::from_millis(50));
        }
        return Ok(());
    }

    let (asr_tx, asr_rx) = mpsc::sync_channel(64);
    ASR_TX
        .set(asr_tx)
        .map_err(|_| "VPM ASR sender was already initialized".to_string())?;
    let (shared_tx, shared_rx) = mpsc::sync_channel(SHARED_AUDIO_QUEUE_CAPACITY);
    SHARED_TX
        .set(shared_tx)
        .map_err(|_| "shared audio sender was already initialized".to_string())?;
    run_audio_loop(
        vpm_process,
        vpm_set_status,
        asr_rx,
        shared_rx,
        options.wake_mode,
        deadline,
    )
}

struct CaptureSession {
    stream: UnixStream,
    deadline: Instant,
    bytes_written: usize,
    callback_count_at_start: u64,
    callback_bytes_at_start: u64,
    type_1_at_start: u64,
    type_2_at_start: u64,
    type_3_at_start: u64,
}

fn open_capture_listener() -> Result<UnixListener, String> {
    match std::fs::symlink_metadata(CAPTURE_SOCKET_PATH) {
        Ok(metadata) if metadata.file_type().is_socket() => {
            std::fs::remove_file(CAPTURE_SOCKET_PATH)
                .map_err(|error| format!("remove stale capture socket failed: {error}"))?;
        }
        Ok(_) => return Err("capture socket path exists and is not a socket".to_string()),
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => return Err(format!("inspect capture socket failed: {error}")),
    }
    let listener = UnixListener::bind(CAPTURE_SOCKET_PATH)
        .map_err(|error| format!("bind capture socket failed: {error}"))?;
    std::fs::set_permissions(CAPTURE_SOCKET_PATH, Permissions::from_mode(0o600))
        .map_err(|error| format!("secure capture socket failed: {error}"))?;
    listener
        .set_nonblocking(true)
        .map_err(|error| format!("set capture listener nonblocking failed: {error}"))?;
    Ok(listener)
}

fn open_shared_audio_listener() -> Result<UnixDatagram, String> {
    match std::fs::symlink_metadata(SHARED_AUDIO_SOCKET_PATH) {
        Ok(metadata) if metadata.file_type().is_socket() => {
            std::fs::remove_file(SHARED_AUDIO_SOCKET_PATH)
                .map_err(|error| format!("remove stale shared audio socket failed: {error}"))?;
        }
        Ok(_) => return Err("shared audio socket path exists and is not a socket".to_string()),
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => return Err(format!("inspect shared audio socket failed: {error}")),
    }
    let listener = UnixDatagram::bind(SHARED_AUDIO_SOCKET_PATH)
        .map_err(|error| format!("bind shared audio socket failed: {error}"))?;
    std::fs::set_permissions(SHARED_AUDIO_SOCKET_PATH, Permissions::from_mode(0o600))
        .map_err(|error| format!("secure shared audio socket failed: {error}"))?;
    listener
        .set_nonblocking(true)
        .map_err(|error| format!("set shared audio socket nonblocking failed: {error}"))?;
    Ok(listener)
}

fn same_pathname_peer(left: &SocketAddr, right: &SocketAddr) -> bool {
    match (left.as_pathname(), right.as_pathname()) {
        (Some(left_path), Some(right_path)) => left_path == right_path,
        _ => false,
    }
}

fn serve_shared_audio_requests(listener: &UnixDatagram, peers: &mut Vec<SocketAddr>) {
    let mut request = [0u8; 64];
    for _ in 0..SHARED_AUDIO_MAX_PEERS {
        match listener.recv_from(&mut request) {
            Ok((size, peer)) => {
                let message = &request[..size];
                if message == SHARED_AUDIO_SUBSCRIBE {
                    if peer.as_pathname().is_none() {
                        continue;
                    }
                    if !peers
                        .iter()
                        .any(|known| same_pathname_peer(known, &peer))
                        && peers.len() < SHARED_AUDIO_MAX_PEERS
                    {
                        peers.push(peer.clone());
                    }
                    match listener.send_to_addr(SHARED_AUDIO_ACK, &peer) {
                        Ok(written) if written == SHARED_AUDIO_ACK.len() => {}
                        // Do not retain a pathname whose datagram endpoint is
                        // already gone.  The next subscriber can immediately
                        // reclaim this bounded peer slot with a fresh SUB.
                        Err(error) if error.kind() == ErrorKind::WouldBlock => {}
                        _ => peers.retain(|known| !same_pathname_peer(known, &peer)),
                    }
                } else if message == SHARED_AUDIO_UNSUBSCRIBE {
                    peers.retain(|known| !same_pathname_peer(known, &peer));
                }
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => break,
            Err(error) if error.kind() == ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
}

fn broadcast_shared_audio(
    listener: &UnixDatagram,
    peers: &mut Vec<SocketAddr>,
    audio: &[u8],
) -> bool {
    if audio.is_empty() || audio.len() > SHARED_AUDIO_MAX_BYTES || audio.len() % 2 != 0 {
        return false;
    }
    let Ok(size) = u32::try_from(audio.len()) else {
        return false;
    };
    let mut frame = Vec::with_capacity(SHARED_AUDIO_MAGIC.len() + 4 + audio.len());
    frame.extend_from_slice(SHARED_AUDIO_MAGIC);
    frame.extend_from_slice(&size.to_le_bytes());
    frame.extend_from_slice(audio);
    let mut delivered = false;
    peers.retain(|peer| match listener.send_to_addr(&frame, peer) {
        Ok(written) => {
            if written == frame.len() {
                delivered = true;
                true
            } else {
                false
            }
        }
        Err(error) if error.kind() == ErrorKind::WouldBlock => true,
        Err(_) => false,
    });
    frame.fill(0);
    delivered
}

fn forward_shared_audio(
    native_trial: &native_trial::NativeTrial,
    render_reference: &mut render_aec::RenderReferenceReceiver,
    listener: &UnixDatagram,
    peers: &mut Vec<SocketAddr>,
    audio: &mut Vec<u8>,
) -> bool {
    let now = Instant::now();
    if native_trial.enabled(now) {
        // `audio` comes solely from the existing 16 kHz VPM enhanced callback.
        // Mode is a short local lease, orthogonal to NAS wake authorization.
        return broadcast_shared_audio(listener, peers, audio);
    }
    if render_reference.render_session_started() {
        // During playback, never fall back to the unprocessed microphone
        // stream: a missing/invalid AEC path must fail closed for barge-in.
        let Ok(residual_frames) = render_reference
            .process_capture_for_broadcast(audio, now)
        else {
            return false;
        };
        let mut delivered = false;
        for mut residual in residual_frames {
            if broadcast_shared_audio(listener, peers, &residual) {
                delivered = true;
            }
            residual.fill(0);
        }
        delivered
    } else {
        broadcast_shared_audio(listener, peers, audio)
    }
}

fn mark_shared_audio_live() -> Result<(), String> {
    if SHARED_AUDIO_LIVE.load(Ordering::Acquire) {
        return Ok(());
    }
    let result = (|| {
        let mut marker = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(O_NOFOLLOW | O_CLOEXEC)
            .open(SHARED_AUDIO_LIVE_PATH)
            .map_err(|error| format!("create shared audio live marker failed: {error}"))?;
        marker
            .write_all(b"FVSH1-OWNER-LIVE\n")
            .map_err(|error| format!("write shared audio live marker failed: {error}"))?;
        marker
            .sync_all()
            .map_err(|error| format!("sync shared audio live marker failed: {error}"))?;
        marker
            .set_permissions(Permissions::from_mode(0o600))
            .map_err(|error| format!("secure shared audio live marker failed: {error}"))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(SHARED_AUDIO_LIVE_PATH);
        return result;
    }
    SHARED_AUDIO_LIVE.store(true, Ordering::Release);
    Ok(())
}

fn open_preroll_listener() -> Result<UnixListener, String> {
    match std::fs::symlink_metadata(PREROLL_SOCKET_PATH) {
        Ok(metadata) if metadata.file_type().is_socket() => {
            std::fs::remove_file(PREROLL_SOCKET_PATH)
                .map_err(|error| format!("remove stale preroll socket failed: {error}"))?;
        }
        Ok(_) => return Err("preroll socket path exists and is not a socket".to_string()),
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => return Err(format!("inspect preroll socket failed: {error}")),
    }
    let listener = UnixListener::bind(PREROLL_SOCKET_PATH)
        .map_err(|error| format!("bind preroll socket failed: {error}"))?;
    std::fs::set_permissions(PREROLL_SOCKET_PATH, Permissions::from_mode(0o600))
        .map_err(|error| format!("secure preroll socket failed: {error}"))?;
    listener
        .set_nonblocking(true)
        .map_err(|error| format!("set preroll listener nonblocking failed: {error}"))?;
    Ok(listener)
}

fn take_preroll() -> Option<Vec<u8>> {
    let pending = PREROLL_PENDING.get()?;
    let Ok(mut pending) = pending.lock() else {
        return None;
    };
    let snapshot = pending.take()?;
    if snapshot.created_at.elapsed() > PREROLL_TTL {
        let mut pcm = snapshot.pcm;
        pcm.fill(0);
        return None;
    }
    Some(snapshot.pcm)
}

fn take_fresh_sherpa_wake_event(contents: &[u8], now_ms: u64, last_event_ms: u64) -> Option<u64> {
    for raw_line in contents.rsplit(|byte| *byte == b'\n') {
        if raw_line.is_empty() {
            continue;
        }
        let line = std::str::from_utf8(raw_line).ok()?;
        let (timestamp, keyword) = line.split_once('@')?;
        if keyword.is_empty() || keyword == "__STARTED__" {
            continue;
        }
        let event_ms = timestamp.parse::<u64>().ok()?;
        let age_ms = now_ms.checked_sub(event_ms)?;
        if event_ms <= last_event_ms || age_ms > PREROLL_TTL.as_millis() as u64 {
            return None;
        }
        return Some(event_ms);
    }
    None
}

fn authorize_sherpa_preroll() -> bool {
    let Ok(mut events) = OpenOptions::new()
        .read(true)
        .custom_flags(O_NOFOLLOW | O_CLOEXEC)
        .open(SHERPA_WAKE_EVENT_PATH)
    else {
        return false;
    };
    let Ok(metadata) = events.metadata() else {
        return false;
    };
    if !metadata.is_file() || metadata.permissions().mode() & 0o777 != 0o600 {
        return false;
    }
    let start = metadata.len().saturating_sub(SHERPA_WAKE_EVENT_SCAN_BYTES);
    if events.seek(SeekFrom::Start(start)).is_err() {
        return false;
    }
    let mut contents = Vec::with_capacity(SHERPA_WAKE_EVENT_SCAN_BYTES as usize);
    if events
        .take(SHERPA_WAKE_EVENT_SCAN_BYTES)
        .read_to_end(&mut contents)
        .is_err()
    {
        return false;
    }
    let Ok(now) = SystemTime::now().duration_since(UNIX_EPOCH) else {
        return false;
    };
    let Ok(now_ms) = u64::try_from(now.as_millis()) else {
        return false;
    };
    let last_event_ms = SHERPA_LAST_PREROLL_EVENT_MS.load(Ordering::Acquire);
    let Some(event_ms) = take_fresh_sherpa_wake_event(&contents, now_ms, last_event_ms) else {
        return false;
    };
    SHERPA_LAST_PREROLL_EVENT_MS.store(event_ms, Ordering::Release);
    true
}

fn write_preroll_response(writer: &mut impl Write, pcm: Option<Vec<u8>>) -> std::io::Result<()> {
    let Some(mut pcm) = pcm else {
        return writer.write_all(PREROLL_EMPTY_RESPONSE);
    };
    let result = (|| {
        let size = u32::try_from(pcm.len())
            .map_err(|_| std::io::Error::new(ErrorKind::InvalidData, "pre-roll is too large"))?;
        writer.write_all(PREROLL_RESPONSE_MAGIC)?;
        writer.write_all(&size.to_le_bytes())?;
        writer.write_all(&pcm)
    })();
    pcm.fill(0);
    result
}

fn serve_preroll_requests(listener: &UnixListener, wake_mode: WakeMode) {
    let Ok((mut stream, _)) = listener.accept() else {
        return;
    };
    // Keep this aligned with the client-side 300 ms request deadline.  A
    // shorter owner timeout can close first and turn a valid wake snapshot
    // into an ambiguous EOF when the OH2P scheduler is busy.
    if stream
        .set_read_timeout(Some(Duration::from_millis(300)))
        .is_err()
        || stream
            .set_write_timeout(Some(Duration::from_millis(250)))
            .is_err()
    {
        return;
    }
    let mut request = [0u8; PREROLL_REQUEST.len()];
    if stream.read_exact(&mut request).is_err() || request.as_slice() != PREROLL_REQUEST {
        return;
    }
    if wake_mode == WakeMode::Sherpa {
        // The Sherpa adapter owns the wake decision, so VPM's native wake
        // callbacks cannot publish this snapshot. Authorize exactly one
        // snapshot from a fresh, private KWS event, then copy the current
        // in-memory ring. Replayed GETs and startup markers are rejected.
        if !authorize_sherpa_preroll() {
            let _ = write_preroll_response(&mut stream, None);
            return;
        }
        publish_preroll();
    }
    let result = write_preroll_response(&mut stream, take_preroll());
    if result.is_err() {
        eprintln!("vpm-owner: preroll client disconnected before upload completed");
    }
}

fn end_capture(
    session: &mut Option<CaptureSession>,
    vpm_set_status: VpmSetStatus,
    wake_mode: WakeMode,
) {
    CAPTURE_ACTIVE.store(false, Ordering::SeqCst);
    if let Some(session) = session.take() {
        let next_status = wake_mode.capture_end_status();
        let result = unsafe { vpm_set_status(next_status) };
        eprintln!(
            "vpm-owner: capture_end bytes_written={} callbacks={} callback_bytes={} type1={} type2={} type3={} next_status={} status_result={}",
            session.bytes_written,
            ASR_CALLBACK_COUNT
                .load(Ordering::Relaxed)
                .saturating_sub(session.callback_count_at_start),
            ASR_CALLBACK_BYTES
                .load(Ordering::Relaxed)
                .saturating_sub(session.callback_bytes_at_start),
            ASR_CALLBACK_TYPE_1
                .load(Ordering::Relaxed)
                .saturating_sub(session.type_1_at_start),
            ASR_CALLBACK_TYPE_2
                .load(Ordering::Relaxed)
                .saturating_sub(session.type_2_at_start),
            ASR_CALLBACK_TYPE_3
                .load(Ordering::Relaxed)
                .saturating_sub(session.type_3_at_start),
            next_status,
            result,
        );
        if result != 0 {
            eprintln!("vpm-owner: vpm_set_status({next_status}) returned {result}");
        }
    }
}

fn run_audio_loop(
    vpm_process: VpmProcess,
    vpm_set_status: VpmSetStatus,
    asr_rx: Receiver<Vec<u8>>,
    shared_rx: Receiver<Vec<u8>>,
    wake_mode: WakeMode,
    deadline: Option<Instant>,
) -> Result<(), String> {
    let listener = open_capture_listener()?;
    let preroll_listener = open_preroll_listener()?;
    let shared_listener = open_shared_audio_listener()?;
    let mut native_trial = native_trial::NativeTrial::bind()
        .map_err(|error| format!("native trial control unavailable: {error}"))?;
    // An unavailable optional probe must never take down the audio owner.
    // Collection is OFF until one authenticated local measurement request.
    #[cfg(feature = "acoustic-probe")]
    let _acoustic_probe = match acoustic_probe::Runtime::start() {
        Ok(runtime) => Some(runtime),
        Err(_) => {
            eprintln!("vpm-owner: acoustic_probe_unavailable");
            None
        }
    };
    // Offline render-reference candidate only. It exposes no capability and
    // does not alter the VPM/capture path until an audited DSP backend proves
    // ERLE, delay/drift and double-talk behavior.
    let mut render_reference = render_aec::RenderReferenceReceiver::bind(
        render_aec::RENDER_REFERENCE_SOCKET_PATH,
        render_aec::AlignmentConfig::default(),
    )
    .map_err(|error| format!("bind render-reference candidate failed: {error}"))?;
    let mut shared_peers = Vec::with_capacity(SHARED_AUDIO_MAX_PEERS);
    let mut capture: Option<CaptureSession> = None;
    let mut command = Command::new("/usr/bin/arecord");
    command
        .args([
            "--quiet",
            "-D",
            "noop",
            "-t",
            "raw",
            "-f",
            "S32_LE",
            "-r",
            &SAMPLE_RATE.to_string(),
            "-c",
            &CHANNELS.to_string(),
            "--buffer-size",
            &BUFFER_SIZE.to_string(),
            "--period-size",
            &PERIOD_SIZE.to_string(),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    unsafe {
        command.pre_exec(|| {
            if prctl(PR_SET_PDEATHSIG, SIGTERM) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            if getppid() == 1 {
                _exit(1);
            }
            Ok(())
        });
    }
    let child = command
        .spawn()
        .map_err(|error| format!("spawn arecord failed: {error}"))?;
    let mut child = ChildGuard(child);
    let mut stdout = child
        .0
        .stdout
        .take()
        .ok_or_else(|| "arecord stdout is unavailable".to_string())?;
    let fd = stdout.as_raw_fd();
    let current_flags = unsafe { fcntl(fd, F_GETFL) };
    if current_flags < 0 || unsafe { fcntl(fd, F_SETFL, current_flags | O_NONBLOCK) } != 0 {
        return Err(format!(
            "set arecord pipe nonblocking failed: {}",
            std::io::Error::last_os_error()
        ));
    }

    let mut buffer = vec![0u8; FRAME_BYTES];
    let mut filled = 0usize;
    let mut frames = 0u64;
    let mut nonzero_process_results = 0u64;
    let mut stream_health = stream_health::StreamHealth::new(
        Instant::now(), ENHANCED_CALLBACK_COUNT.load(Ordering::Relaxed));
    while !should_stop(deadline) {
        native_trial.poll(Instant::now());
        render_reference.poll(Instant::now());
        serve_shared_audio_requests(&shared_listener, &mut shared_peers);
        while let Ok(mut audio) = shared_rx.try_recv() {
            if forward_shared_audio(
                &native_trial,
                &mut render_reference,
                &shared_listener,
                &mut shared_peers,
                &mut audio,
            ) {
                mark_shared_audio_live()?;
            }
            audio.fill(0);
        }
        match stdout.read(&mut buffer[filled..]) {
            Ok(0) => return Err("arecord stdout closed".to_string()),
            Ok(read) => {
                filled += read;
                if filled != buffer.len() {
                    continue;
                }
                let input = VpmInput {
                    size: buffer.len() as u32,
                    reserved_04: 0,
                    start_time_ms: frames.saturating_mul(u64::from(FRAME_MS)),
                    data_frame_number: frames,
                    frame_size: buffer.len() as u32,
                    data: buffer.as_ptr(),
                };
                #[cfg(feature = "acoustic-probe")]
                acoustic_probe::tap_raw(&buffer);
                let process_result = unsafe { vpm_process(&input) };
                if process_result != 0 {
                    nonzero_process_results = nonzero_process_results.saturating_add(1);
                    if nonzero_process_results <= 3 {
                        eprintln!(
                            "vpm-owner: vpm_process frame {frames} returned {process_result}"
                        );
                    }
                }
                frames = frames.saturating_add(1);
                filled = 0;
                // VPM can stop enhanced callbacks while ALSA and the owner remain
                // alive. Reuse the same status-2 operation as the capture socket.
                // No new capture owner, raw fallback, or periodic healthy reset.
                if stream_health.needs_rearm(Instant::now(),
                    ENHANCED_CALLBACK_COUNT.load(Ordering::Relaxed),
                    wake_mode == WakeMode::Sherpa && capture.is_none()) {
                    let result = unsafe { vpm_set_status(2) };
                    eprintln!("vpm-owner: enhanced_stream_stalled raw_frames={frames} rearm_status2={result}");
                }

                // A wake callback snapshots the already-buffered middle stream.
                // The edge client may consume that one snapshot once, after its
                // signed wake event is authorized; no PCM is written to disk.
                serve_preroll_requests(&preroll_listener, wake_mode);
                serve_shared_audio_requests(&shared_listener, &mut shared_peers);
                while let Ok(mut audio) = shared_rx.try_recv() {
                    if forward_shared_audio(
                        &native_trial,
                        &mut render_reference,
                        &shared_listener,
                        &mut shared_peers,
                        &mut audio,
                    ) {
                        mark_shared_audio_live()?;
                    }
                    audio.fill(0);
                }

                if capture.is_none() {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            stream
                                .set_write_timeout(Some(Duration::from_millis(250)))
                                .map_err(|error| {
                                    format!("set capture write timeout failed: {error}")
                                })?;
                            while asr_rx.try_recv().is_ok() {}
                            CAPTURE_ACTIVE.store(true, Ordering::SeqCst);
                            let result = unsafe { vpm_set_status(2) };
                            if result != 0 {
                                CAPTURE_ACTIVE.store(false, Ordering::SeqCst);
                                return Err(format!("vpm_set_status(2) returned {result}"));
                            }
                            capture = Some(CaptureSession {
                                stream,
                                deadline: Instant::now() + CAPTURE_WINDOW,
                                bytes_written: 0,
                                callback_count_at_start: ASR_CALLBACK_COUNT.load(Ordering::Relaxed),
                                callback_bytes_at_start: ASR_CALLBACK_BYTES.load(Ordering::Relaxed),
                                type_1_at_start: ASR_CALLBACK_TYPE_1.load(Ordering::Relaxed),
                                type_2_at_start: ASR_CALLBACK_TYPE_2.load(Ordering::Relaxed),
                                type_3_at_start: ASR_CALLBACK_TYPE_3.load(Ordering::Relaxed),
                            });
                            eprintln!("vpm-owner: capture_start status2_result={result}");
                        }
                        Err(error) if error.kind() == ErrorKind::WouldBlock => {}
                        Err(error) => return Err(format!("accept capture socket failed: {error}")),
                    }
                }

                let mut capture_failed = false;
                while let Ok(mut audio) = asr_rx.try_recv() {
                    if let Some(session) = capture.as_mut() {
                        let remaining = MAX_CAPTURE_BYTES.saturating_sub(session.bytes_written);
                        let write_len = audio.len().min(remaining);
                        if write_len > 0 {
                            if session.stream.write_all(&audio[..write_len]).is_err() {
                                capture_failed = true;
                            } else {
                                session.bytes_written += write_len;
                            }
                        }
                    }
                    audio.fill(0);
                }
                let capture_finished = capture
                    .as_ref()
                    .map(|session| {
                        capture_failed
                            || Instant::now() >= session.deadline
                            || session.bytes_written >= MAX_CAPTURE_BYTES
                    })
                    .unwrap_or(false);
                if capture_finished {
                    end_capture(&mut capture, vpm_set_status, wake_mode);
                }
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(2));
            }
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(error) => return Err(format!("read arecord failed: {error}")),
        }
    }
    end_capture(&mut capture, vpm_set_status, wake_mode);
    let _ = std::fs::remove_file(CAPTURE_SOCKET_PATH);
    let _ = std::fs::remove_file(PREROLL_SOCKET_PATH);
    let _ = std::fs::remove_file(SHARED_AUDIO_SOCKET_PATH);
    let _ = std::fs::remove_file(SHARED_AUDIO_LIVE_PATH);
    SHARED_AUDIO_LIVE.store(false, Ordering::Release);
    eprintln!(
        "vpm-owner: processed_frames={frames} nonzero_process_results={nonzero_process_results}"
    );
    Ok(())
}

fn should_stop(deadline: Option<Instant>) -> bool {
    STOP.load(Ordering::SeqCst)
        || WRITER_FAILED.load(Ordering::SeqCst)
        || deadline
            .map(|value| Instant::now() >= value)
            .unwrap_or(false)
}

fn write_ptr(buffer: &mut [u8], offset: usize, pointer: usize) -> Result<(), String> {
    let value = u32::try_from(pointer).map_err(|_| "pointer does not fit u32".to_string())?;
    write_u32(buffer, offset, value)
}

fn write_u32(buffer: &mut [u8], offset: usize, value: u32) -> Result<(), String> {
    let end = offset
        .checked_add(4)
        .ok_or_else(|| "offset overflow".to_string())?;
    let destination = buffer
        .get_mut(offset..end)
        .ok_or_else(|| format!("offset 0x{offset:x} is outside init buffer"))?;
    destination.copy_from_slice(&value.to_ne_bytes());
    Ok(())
}

fn write_f32(buffer: &mut [u8], offset: usize, value: f32) -> Result<(), String> {
    write_u32(buffer, offset, value.to_bits())
}

#[cfg(test)]
mod preroll_tests {
    use super::{
        append_bounded_audio, take_fresh_sherpa_wake_event, write_preroll_response,
        PREROLL_EMPTY_RESPONSE, PREROLL_MAX_BYTES, PREROLL_RESPONSE_MAGIC,
    };
    use std::collections::VecDeque;

    #[test]
    fn preroll_ring_is_bounded_and_keeps_latest_pcm() {
        let mut ring = VecDeque::new();
        let first = vec![1u8; PREROLL_MAX_BYTES - 2];
        append_bounded_audio(&mut ring, &first);
        append_bounded_audio(&mut ring, &[2, 3, 4, 5]);

        assert_eq!(ring.len(), PREROLL_MAX_BYTES);
        assert_eq!(ring.front(), Some(&1));
        assert_eq!(ring.iter().rev().take(4).copied().collect::<Vec<_>>(), vec![5, 4, 3, 2]);
    }

    #[test]
    fn preroll_ring_accepts_empty_audio_without_growth() {
        let mut ring = VecDeque::from([7u8, 8]);
        append_bounded_audio(&mut ring, &[]);
        assert_eq!(ring.into_iter().collect::<Vec<_>>(), vec![7, 8]);
    }

    #[test]
    fn sherpa_event_authorizes_one_fresh_non_startup_snapshot() {
        let events = b"1000@__STARTED__\n2500@\xe5\x91\xa8\xe5\x85\xad\n";
        assert_eq!(take_fresh_sherpa_wake_event(events, 3000, 0), Some(2500));
        assert_eq!(take_fresh_sherpa_wake_event(events, 3000, 2500), None);
        assert_eq!(take_fresh_sherpa_wake_event(events, 6001, 0), None);
    }

    #[test]
    fn sherpa_event_rejects_startup_future_and_malformed_lines() {
        assert_eq!(
            take_fresh_sherpa_wake_event(b"1000@__STARTED__\n", 1200, 0),
            None
        );
        assert_eq!(
            take_fresh_sherpa_wake_event(b"bad@\xe5\x91\xa8\xe5\x85\xad\n", 1200, 0),
            None
        );
        assert_eq!(
            take_fresh_sherpa_wake_event(b"2000@\xe5\x91\xa8\xe5\x85\xad\n", 1200, 0),
            None
        );
    }

    #[test]
    fn empty_preroll_has_an_explicit_protocol_response_instead_of_eof() {
        let mut response = Vec::new();
        write_preroll_response(&mut response, None).expect("empty response");
        assert_eq!(response, PREROLL_EMPTY_RESPONSE);
    }

    #[test]
    fn pcm_preroll_has_magic_length_and_payload() {
        let mut response = Vec::new();
        write_preroll_response(&mut response, Some(vec![1, 2, 3, 4])).expect("PCM response");
        assert_eq!(&response[..PREROLL_RESPONSE_MAGIC.len()], PREROLL_RESPONSE_MAGIC);
        assert_eq!(
            &response[PREROLL_RESPONSE_MAGIC.len()..PREROLL_RESPONSE_MAGIC.len() + 4],
            &4u32.to_le_bytes()
        );
        assert_eq!(&response[PREROLL_RESPONSE_MAGIC.len() + 4..], &[1, 2, 3, 4]);
    }
}

#[cfg(test)]
mod shared_audio_tests {
    use super::{
        broadcast_shared_audio, same_pathname_peer, serve_shared_audio_requests,
        SHARED_AUDIO_SUBSCRIBE,
    };
    use std::os::unix::net::UnixDatagram;

    #[test]
    fn pathname_peers_compare_by_socket_path() {
        let unnamed = UnixDatagram::unbound().expect("create unnamed datagram");
        let unnamed_address = unnamed.local_addr().expect("read unnamed address");
        assert!(!same_pathname_peer(&unnamed_address, &unnamed_address));

        let unique = format!(
            "fvs-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock after epoch")
                .as_nanos()
        );
        // macOS sockaddr_un.sun_path is much shorter than Linux. Keep these
        // regression sockets under the real /tmp path instead of its long
        // per-user std::env::temp_dir() location.
        let server_path = std::path::Path::new("/tmp").join(format!("{unique}-s"));
        let client_path = std::path::Path::new("/tmp").join(format!("{unique}-c"));
        let other_path = std::path::Path::new("/tmp").join(format!("{unique}-o"));
        let server = UnixDatagram::bind(&server_path).expect("bind server datagram");
        let client = UnixDatagram::bind(&client_path).expect("bind client datagram");
        let other = UnixDatagram::bind(&other_path).expect("bind other datagram");
        client.send_to(b"a", &server_path).expect("send client datagram");
        let mut byte = [0u8; 1];
        let (_, client_address) = server.recv_from(&mut byte).expect("receive client datagram");
        other.send_to(b"b", &server_path).expect("send other datagram");
        let (_, other_address) = server.recv_from(&mut byte).expect("receive other datagram");

        assert!(same_pathname_peer(&client_address, &client_address));
        assert!(!same_pathname_peer(&client_address, &other_address));

        drop((server, client, other));
        for path in [&server_path, &client_path, &other_path] {
            std::fs::remove_file(path).expect("remove test socket");
        }
    }

    #[test]
    fn stale_peer_is_reclaimed_after_fresh_subscription_without_owner_restart() {
        let unique = format!(
            "fvs-reclaim-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock after epoch")
                .as_nanos()
        );
        let server_path = std::path::Path::new("/tmp").join(format!("{unique}-s"));
        let client_path = std::path::Path::new("/tmp").join(format!("{unique}-c"));
        let server = UnixDatagram::bind(&server_path).expect("bind server datagram");
        server
            .set_nonblocking(true)
            .expect("set server nonblocking");
        let client = UnixDatagram::bind(&client_path).expect("bind client datagram");
        client
            .send_to(SHARED_AUDIO_SUBSCRIBE, &server_path)
            .expect("initial subscribe");
        let mut peers = Vec::new();
        serve_shared_audio_requests(&server, &mut peers);
        assert_eq!(peers.len(), 1);
        let mut ack = [0u8; 16];
        assert_eq!(client.recv(&mut ack).expect("initial ack"), 9);

        drop(client);
        std::fs::remove_file(&client_path).expect("remove stale subscriber");
        assert!(!broadcast_shared_audio(&server, &mut peers, &[1, 2]));
        assert!(peers.is_empty(), "dead pathname peer must be evicted");

        let replacement = UnixDatagram::bind(&client_path).expect("rebind subscriber path");
        replacement
            .send_to(SHARED_AUDIO_SUBSCRIBE, &server_path)
            .expect("renewed subscribe");
        serve_shared_audio_requests(&server, &mut peers);
        assert_eq!(peers.len(), 1);
        assert_eq!(replacement.recv(&mut ack).expect("renewed ack"), 9);
        assert!(broadcast_shared_audio(&server, &mut peers, &[3, 4]));
        let mut frame = [0u8; 64];
        let received = replacement.recv(&mut frame).expect("renewed PCM");
        assert!(received > 14);
        assert_eq!(&frame[..10], b"FVSH1 PCM\n");

        drop((server, replacement));
        for path in [&server_path, &client_path] {
            std::fs::remove_file(path).expect("remove test socket");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{is_real_wakeup_data, WakeMode};

    #[test]
    fn only_firmware_real_wakeup_data_is_accepted() {
        assert!(is_real_wakeup_data(0));
        assert!(!is_real_wakeup_data(1));
        assert!(!is_real_wakeup_data(-1));
    }

    #[test]
    fn wake_modes_keep_native_events_and_streaming_status_separate() {
        assert_eq!(WakeMode::Vpm.initial_status(), 6);
        assert!(WakeMode::Vpm.publishes_vpm_wake());
        assert_eq!(WakeMode::Vpm.capture_end_status(), 3);

        assert_eq!(WakeMode::Sherpa.initial_status(), 2);
        assert!(!WakeMode::Sherpa.publishes_vpm_wake());
        assert_eq!(WakeMode::Sherpa.capture_end_status(), 2);
    }
}

fn dl_error() -> String {
    let pointer = unsafe { dlerror() };
    if pointer.is_null() {
        "unknown dynamic loader error".to_string()
    } else {
        unsafe { CStr::from_ptr(pointer) }
            .to_string_lossy()
            .into_owned()
    }
}
