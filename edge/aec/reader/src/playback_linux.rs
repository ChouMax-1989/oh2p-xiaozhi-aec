// SPDX-License-Identifier: MIT
use super::{has_sound, Activity, Format};
use std::{
    env, fs,
    io::{self, Read},
    os::{
        fd::{AsRawFd, FromRawFd, RawFd},
        raw::{c_int, c_ulong},
        unix::{net::UnixDatagram, process::CommandExt},
    },
    path::Path,
    process::{self, Command},
    thread,
    time::{Duration, Instant},
};

#[path = "playback_alsa.rs"]
mod alsa;

const ENDPOINT: &str = "/tmp/open-xiaoai/tts-playback.sock";
const CHECK_INTERVAL: Duration = Duration::from_millis(10);
const STALL_LIMIT: Duration = Duration::from_secs(5);
const POLLIN: i16 = 1;
const POLLNVAL: i16 = 32;

#[repr(C)]
struct PollFd {
    fd: c_int,
    events: i16,
    revents: i16,
}

extern "C" {
    fn poll(fds: *mut PollFd, count: c_ulong, timeout: c_int) -> c_int;
}

fn input_ready(input: RawFd, wait: Option<Duration>) -> io::Result<bool> {
    let mut fd = PollFd {
        fd: input,
        events: POLLIN,
        revents: 0,
    };
    let timeout = wait.map_or(-1, |duration| {
        duration.as_nanos().div_ceil(1_000_000) as c_int
    });
    if unsafe { poll(&mut fd, 1, timeout) } < 0 {
        let error = io::Error::last_os_error();
        return if error.kind() == io::ErrorKind::Interrupted {
            Ok(false)
        } else {
            Err(error)
        };
    }
    if fd.revents & POLLNVAL != 0 {
        return Err(io::Error::other("invalid player stdin"));
    }
    // POLLHUP also needs a read to collect any final bytes and detect EOF.
    Ok(fd.revents != 0)
}

struct Notice<'a> {
    endpoint: &'a Path,
    identity: Option<(UnixDatagram, u64)>,
    seq: u64,
    on: bool,
}

fn start_ticks(stat: &[u8]) -> Option<u64> {
    // comm (field 2) may contain whitespace, closing parentheses or non-UTF8.
    let tail = &stat[stat.iter().rposition(|&b| b == b')')? + 1..];
    let ticks = tail
        .split(u8::is_ascii_whitespace)
        .filter(|p| !p.is_empty())
        .nth(19)?;
    std::str::from_utf8(ticks).ok()?.parse().ok()
}

impl<'a> Notice<'a> {
    fn new(endpoint: &'a Path) -> Self {
        let identity = (|| {
            let mut stat = [0; 1024];
            let count = fs::File::open("/proc/self/stat")
                .ok()?
                .read(&mut stat)
                .ok()?;
            let ticks = start_ticks(&stat[..count])?;
            let socket = UnixDatagram::unbound().ok()?;
            socket.set_nonblocking(true).ok()?;
            Some((socket, ticks))
        })();
        Self {
            endpoint,
            identity,
            seq: 0,
            on: false,
        }
    }

    fn send(&mut self, on: bool) {
        self.on = on;
        if let Some((socket, ticks)) = &self.identity {
            self.seq += 1;
            let message = format!(
                "TTS1 {} {} {} {}\n",
                process::id(),
                ticks,
                self.seq,
                if on { "ON" } else { "OFF" }
            );
            // Resolve the pathname each time so an owner restart recovers on heartbeat.
            let _ = socket.send_to(message.as_bytes(), self.endpoint);
        }
    }
}

impl Drop for Notice<'_> {
    fn drop(&mut self) {
        if self.on {
            self.send(false);
        }
    }
}

enum Progress<T> {
    Ready(T),
    Retry,
    Reset,
}

enum Delay {
    Queued(usize),
    Prepared,
    Unknown,
    Reset,
}

trait Pcm {
    fn write(&mut self, samples: &[u8]) -> io::Result<Progress<usize>>;
    fn delay(&mut self) -> Delay;
    fn start(&mut self) -> io::Result<Progress<()>>;
    fn drain(&mut self) -> io::Result<Progress<()>>;
}

fn play(
    input: &mut (impl Read + AsRawFd),
    pcm: &mut impl Pcm,
    format: Format,
    mut notify: impl FnMut(bool),
) -> io::Result<()> {
    let start_wait =
        Duration::from_nanos(u64::from(format.buffer) * 1_000_000_000 / u64::from(format.rate));
    let mut samples = vec![0; format.period as usize * 2];
    let mut filled = 0;
    let mut sent = 0;
    let mut carry = None;
    let mut wrote = false;
    let mut eof = false;
    let mut drained = false;
    let mut activity = Activity::default();
    let mut last_read = Instant::now();
    let mut next_check = last_read;
    let mut last_delay = None;
    let mut stalled = None;

    loop {
        let now = Instant::now();
        // As with native aplay's final short period, pad only after EOF or an
        // input gap. Never discard a complete frame or replay accepted bytes.
        if filled % 2 == 1 && eof {
            filled -= 1;
        }
        let pad = filled >= 2 && (eof || now.duration_since(last_read) >= start_wait);
        if pad && filled < samples.len() {
            let whole = filled & !1;
            carry = (filled > whole).then(|| samples[whole]);
            samples[whole..].fill(0);
            // A half sample remains outside this period until its second byte
            // arrives. A half sample at EOF is not a playable PCM frame.
            filled = samples.len();
        }

        if now >= next_check {
            next_check = now + CHECK_INTERVAL;
            if eof && filled == 0 && !drained {
                if wrote {
                    drained = match pcm.drain()? {
                        Progress::Ready(()) | Progress::Reset => true,
                        Progress::Retry => false,
                    };
                } else {
                    drained = true;
                }
                if drained {
                    activity.queue_ended();
                }
            } else if activity.queued_sound() {
                match pcm.delay() {
                    Delay::Queued(frames) => {
                        if last_delay.is_none_or(|previous| frames < previous) {
                            if last_delay.is_some() {
                                stalled = None;
                            }
                            last_delay = Some(frames);
                        }
                        activity.observe_delay(frames);
                    }
                    Delay::Reset => {
                        activity.queue_ended();
                        last_delay = None;
                    }
                    Delay::Prepared if now.duration_since(last_read) >= start_wait => {
                        if let Progress::Reset = pcm.start()? {
                            activity.queue_ended();
                        }
                    }
                    Delay::Prepared | Delay::Unknown => {}
                }
            }
            if let Some(on) = activity.tick(has_sound(&samples[sent..filled]), now) {
                notify(on);
            }
        }
        if drained && !activity.active() {
            return Ok(());
        }

        if filled == samples.len() || activity.queued_sound() || (eof && !drained) {
            if now.duration_since(*stalled.get_or_insert(now)) >= STALL_LIMIT {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "ALSA playback made no progress",
                ));
            }
        } else {
            stalled = None;
        }

        if filled == samples.len() {
            if activity.before_write(has_sound(&samples[sent..filled]), now) {
                notify(true);
            }
            match pcm.write(&samples[sent..filled])? {
                Progress::Ready(frames) if frames > 0 && frames <= (filled - sent) / 2 => {
                    let end = sent + frames * 2;
                    activity.accepted(&samples[sent..end]);
                    wrote = true;
                    last_delay = None;
                    stalled = None;
                    if end == filled {
                        filled = 0;
                        sent = 0;
                        if let Some(byte) = carry.take() {
                            samples[0] = byte;
                            filled = 1;
                        }
                    } else {
                        sent = end;
                    }
                    continue;
                }
                Progress::Ready(_) => return Err(io::Error::other("invalid ALSA write count")),
                Progress::Reset => {
                    activity.queue_ended();
                    last_delay = None;
                    // ALSA cleared its frame pointer. Keep the unaccepted
                    // suffix and align it to a period in the new queue.
                    samples.copy_within(sent..filled, 0);
                    samples[filled - sent..].fill(0);
                    sent = 0;
                }
                Progress::Retry => {}
            }
            thread::sleep(CHECK_INTERVAL);
        } else if eof {
            thread::sleep(CHECK_INTERVAL);
        } else {
            let wait = (activity.active() || filled >= 2)
                .then(|| next_check.saturating_duration_since(now));
            if input_ready(input.as_raw_fd(), wait)? {
                match input.read(&mut samples[filled..]) {
                    Ok(0) => eof = true,
                    Ok(count) => {
                        filled += count;
                        last_read = Instant::now();
                    }
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                    Err(error) => return Err(error),
                }
            }
        }
    }
}

fn with_pcm<T: Pcm>(
    open: impl FnOnce() -> io::Result<(T, Format)>,
    direct: impl FnOnce(T, Format) -> io::Result<()>,
    fallback: impl FnOnce() -> io::Result<()>,
) -> io::Result<()> {
    match open() {
        Ok((pcm, format)) => direct(pcm, format),
        Err(error) => {
            eprintln!("aplay adapter: ALSA initialization failed, using system aplay: {error}");
            fallback()
        }
    }
}

fn exec_system(args: &[std::ffi::OsString]) -> io::Result<()> {
    let error = Command::new("/usr/bin/aplay").args(args).exec();
    if error.kind() == io::ErrorKind::NotFound {
        Err(Command::new("/bin/aplay").args(args).exec())
    } else {
        Err(error)
    }
}

pub fn run() -> io::Result<()> {
    let args: Vec<_> = env::args_os().skip(1).collect();
    let strings: Option<Vec<_>> = args
        .iter()
        .map(|arg| arg.to_str().map(str::to_owned))
        .collect();
    let Some(format) = strings.as_ref().and_then(|args| Format::from_args(args)) else {
        return exec_system(&args);
    };

    with_pcm(
        || alsa::Alsa::open(format),
        |mut pcm, format| {
            let mut notice = Notice::new(Path::new(ENDPOINT));
            let mut input = unsafe { fs::File::from_raw_fd(0) };
            let result = play(&mut input, &mut pcm, format, |on| notice.send(on));
            // If playback failed, close the PCM before reporting OFF.
            drop(pcm);
            result
        },
        || exec_system(&args),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        cell::RefCell,
        collections::VecDeque,
        io::Write,
        os::unix::net::UnixStream,
        sync::{
            atomic::{AtomicBool, AtomicUsize, Ordering::SeqCst},
            mpsc, Arc,
        },
    };

    const FIXTURE: Format = Format {
        rate: 1000,
        buffer: 60,
        period: 4,
    };
    const WAIT: Duration = Duration::from_secs(2);
    const PREPARED: usize = usize::MAX - 1;

    struct FakePcm {
        accepted: mpsc::Sender<Vec<u8>>,
        remaining: Arc<AtomicUsize>,
        drained: Arc<AtomicBool>,
        writes: VecDeque<Progress<usize>>,
    }

    impl Pcm for FakePcm {
        fn write(&mut self, samples: &[u8]) -> io::Result<Progress<usize>> {
            let progress = self
                .writes
                .pop_front()
                .unwrap_or(Progress::Ready(samples.len() / 2));
            if let Progress::Ready(frames) = progress {
                self.accepted.send(samples[..frames * 2].to_vec()).unwrap();
            }
            Ok(progress)
        }

        fn delay(&mut self) -> Delay {
            match self.remaining.load(SeqCst) {
                usize::MAX => Delay::Unknown,
                PREPARED => Delay::Prepared,
                frames => Delay::Queued(frames),
            }
        }

        fn start(&mut self) -> io::Result<Progress<()>> {
            self.remaining.store(FIXTURE.period as usize, SeqCst);
            Ok(Progress::Ready(()))
        }

        fn drain(&mut self) -> io::Result<Progress<()>> {
            Ok(if self.drained.load(SeqCst) {
                Progress::Ready(())
            } else {
                Progress::Retry
            })
        }
    }

    #[test]
    fn slow_playback_and_eof_wait_for_device_progress() {
        let (mut writer, mut input) = UnixStream::pair().unwrap();
        let remaining = Arc::new(AtomicUsize::new(usize::MAX));
        let drained = Arc::new(AtomicBool::new(false));
        let (accepted, bytes) = mpsc::channel();
        let (notifier, edges) = mpsc::channel();
        let mut pcm = FakePcm {
            accepted,
            remaining: remaining.clone(),
            drained: drained.clone(),
            writes: VecDeque::from([Progress::Retry, Progress::Ready(2), Progress::Reset]),
        };
        let player = thread::spawn(move || {
            play(&mut input, &mut pcm, FIXTURE, |on| {
                notifier.send(on).unwrap()
            })
            .unwrap();
        });
        writer.write_all(&[1, 2, 3, 4, 5, 6, 7, 8]).unwrap();
        assert_eq!(edges.recv_timeout(WAIT), Ok(true));
        assert_eq!(bytes.recv_timeout(WAIT).unwrap(), [1, 2, 3, 4]);
        assert_eq!(bytes.recv_timeout(WAIT).unwrap(), [5, 6, 7, 8, 0, 0, 0, 0]);
        assert_eq!(
            edges.recv_timeout(Duration::from_millis(190)),
            Err(mpsc::RecvTimeoutError::Timeout)
        );
        remaining.store(4, SeqCst);
        assert_eq!(
            edges.recv_timeout(Duration::from_millis(100)),
            Err(mpsc::RecvTimeoutError::Timeout)
        );
        remaining.store(0, SeqCst);
        assert_eq!(edges.recv_timeout(WAIT), Ok(false));

        remaining.store(4, SeqCst);
        writer.write_all(&[0, 9, 10, 11]).unwrap();
        drop(writer);
        assert_eq!(edges.recv_timeout(WAIT), Ok(true));
        assert_eq!(
            bytes.recv_timeout(WAIT).unwrap(),
            [0, 9, 10, 11, 0, 0, 0, 0]
        );
        assert_eq!(
            edges.recv_timeout(Duration::from_millis(160)),
            Err(mpsc::RecvTimeoutError::Timeout)
        );
        drained.store(true, SeqCst);
        assert_eq!(edges.recv_timeout(WAIT), Ok(false));
        player.join().unwrap();
    }

    #[test]
    fn read_fragment_survives_an_idle_period_flush() {
        let (mut writer, mut input) = UnixStream::pair().unwrap();
        let (accepted, bytes) = mpsc::channel();
        let mut pcm = FakePcm {
            accepted,
            remaining: Arc::new(AtomicUsize::new(0)),
            drained: Arc::new(AtomicBool::new(true)),
            writes: VecDeque::new(),
        };
        let player = thread::spawn(move || play(&mut input, &mut pcm, FIXTURE, |_| {}).unwrap());
        writer.write_all(&[1, 2, 3]).unwrap();
        assert_eq!(bytes.recv_timeout(WAIT).unwrap(), [1, 2, 0, 0, 0, 0, 0, 0]);
        writer.write_all(&[4, 5, 6, 7]).unwrap();
        drop(writer);
        assert_eq!(bytes.recv_timeout(WAIT).unwrap(), [3, 4, 5, 6, 0, 0, 0, 0]);
        player.join().unwrap();
    }

    #[test]
    fn short_burst_starts_without_waiting_for_eof() {
        let (mut writer, mut input) = UnixStream::pair().unwrap();
        let remaining = Arc::new(AtomicUsize::new(PREPARED));
        let (accepted, bytes) = mpsc::channel();
        let (notifier, edges) = mpsc::channel();
        let mut pcm = FakePcm {
            accepted,
            remaining: remaining.clone(),
            drained: Arc::new(AtomicBool::new(true)),
            writes: VecDeque::new(),
        };
        let player = thread::spawn(move || {
            play(&mut input, &mut pcm, FIXTURE, |on| {
                notifier.send(on).unwrap()
            })
            .unwrap();
        });
        writer.write_all(&[1, 2]).unwrap();
        assert_eq!(edges.recv_timeout(WAIT), Ok(true));
        assert_eq!(bytes.recv_timeout(WAIT).unwrap(), [1, 2, 0, 0, 0, 0, 0, 0]);
        let deadline = Instant::now() + WAIT;
        while remaining.load(SeqCst) == PREPARED && Instant::now() < deadline {
            thread::sleep(CHECK_INTERVAL);
        }
        assert_eq!(remaining.load(SeqCst), FIXTURE.period as usize);
        remaining.store(0, SeqCst);
        assert_eq!(edges.recv_timeout(WAIT), Ok(false));
        drop(writer);
        player.join().unwrap();
    }

    #[test]
    fn initialization_fallback_sees_untouched_input() {
        let (mut writer, input) = UnixStream::pair().unwrap();
        writer.write_all(&[1, 2, 3, 4]).unwrap();
        drop(writer);
        let input = RefCell::new(input);
        with_pcm::<FakePcm>(
            || Err(io::Error::other("fixture open failure")),
            |_, _| panic!("direct playback consumed input before fallback"),
            || {
                let mut bytes = Vec::new();
                input.borrow_mut().read_to_end(&mut bytes)?;
                assert_eq!(bytes, [1, 2, 3, 4]);
                Ok(())
            },
        )
        .unwrap();
    }

    #[test]
    fn stat_uses_process_leader_start_ticks() {
        let stat = fs::read("/proc/self/stat").unwrap();
        assert!(start_ticks(&stat).is_some());
        let hostile_comm = b"43 (x) \xff y)) S 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 876 99";
        assert_eq!(start_ticks(hostile_comm), Some(876));
    }
}
