// SPDX-License-Identifier: MIT
use super::{Activity, Format};
use std::{
    env, fs,
    io::{self, Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        raw::{c_int, c_ulong},
        unix::{
            net::UnixDatagram,
            process::{CommandExt, ExitStatusExt},
        },
    },
    path::Path,
    process::{self, Child, ChildStdin, Command, Stdio},
    time::Instant,
};

const ENDPOINT: &str = "/tmp/open-xiaoai/tts-playback.sock";
const CHUNK: usize = 2048; // <= Linux PIPE_BUF, at most one chunk held by the proxy
const POLLIN: i16 = 1;
const POLLOUT: i16 = 4;
const PIPE_ERROR: i16 = 8 | 16 | 32; // POLLERR | POLLHUP | POLLNVAL

#[repr(C)]
struct PollFd {
    fd: c_int,
    events: i16,
    revents: i16,
}

extern "C" {
    fn poll(fds: *mut PollFd, count: c_ulong, timeout: c_int) -> c_int;
    fn prctl(option: c_int, ...) -> c_int;
    fn getppid() -> c_int;
    fn fcntl(fd: c_int, cmd: c_int, ...) -> c_int;
}

struct Notice<'a> {
    endpoint: &'a Path,
    identity: Option<(UnixDatagram, u64)>,
    seq: u64,
}

fn start_ticks(stat: &str) -> Option<u64> {
    // comm (field 2) may itself contain spaces or a closing parenthesis.
    stat.rsplit_once(')')?
        .1
        .split_whitespace()
        .nth(19)?
        .parse()
        .ok()
}

impl<'a> Notice<'a> {
    fn new(endpoint: &'a Path) -> Self {
        let identity = (|| {
            let pid = process::id();
            // Identify the process leader even when the caller runs on a helper thread.
            let stat = fs::read_to_string(format!("/proc/{pid}/task/{pid}/stat")).ok()?;
            let ticks = start_ticks(&stat)?;
            let sock = UnixDatagram::unbound().ok()?;
            sock.set_nonblocking(true).ok()?;
            Some((sock, ticks))
        })();
        Self {
            endpoint,
            identity,
            seq: 0,
        }
    }

    fn send(&mut self, on: bool) {
        if let Some((sock, ticks)) = &self.identity {
            self.seq += 1;
            let message = format!(
                "TTS1 {} {} {} {}\n",
                process::id(),
                ticks,
                self.seq,
                if on { "ON" } else { "OFF" }
            );
            // Resolve the pathname each time so an owner restart recovers on heartbeat.
            let _ = sock.send_to(message.as_bytes(), self.endpoint);
        }
    }
}

fn spawn_player(command: &mut Command, expected_parent: u32) -> io::Result<Child> {
    // Only async-signal-safe system calls run between fork and exec. Register
    // first, then catch parent death between fork and prctl (including reparenting).
    unsafe {
        command.pre_exec(move || {
            if prctl(1, 9 as c_ulong, 0 as c_ulong, 0 as c_ulong, 0 as c_ulong) < 0 {
                return Err(io::Error::last_os_error());
            }
            if getppid() as u32 != expected_parent {
                return Err(io::Error::from_raw_os_error(3)); // ESRCH
            }
            Ok(())
        });
    }
    command.stdin(Stdio::piped()).spawn()
}

fn relay(
    input: &mut fs::File,
    output: &mut ChildStdin,
    activity: &mut Activity,
    notice: &mut Notice<'_>,
) -> io::Result<()> {
    let mut bytes = [0u8; CHUNK];
    let mut used = 0;
    let mut pending = 0;
    let mut eof = false;
    loop {
        let now = Instant::now();
        if let Some(on) = activity.tick(now) {
            notice.send(on);
        }
        if eof && pending == 0 {
            return Ok(());
        }
        let timeout = activity.deadline().map_or(-1, |when| {
            when.saturating_duration_since(now)
                .as_nanos()
                .div_ceil(1_000_000) as c_int
        });
        let mut fds = [
            PollFd {
                fd: if pending == 0 { input.as_raw_fd() } else { -1 },
                events: POLLIN,
                revents: 0,
            },
            PollFd {
                fd: output.as_raw_fd(),
                events: if pending == 0 { 0 } else { POLLOUT },
                revents: 0,
            },
        ];
        if unsafe { poll(fds.as_mut_ptr(), 2, timeout) } < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(err);
        }
        if fds[1].revents & PIPE_ERROR != 0 {
            return Err(io::Error::from(io::ErrorKind::BrokenPipe));
        }
        if fds[0].revents != 0 {
            let count = match input.read(&mut bytes[used..]) {
                Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                result => result?,
            };
            eof = count == 0;
            used += count;
            // Carry half a sample across fragmented pipe reads. Preserve even
            // a final incomplete sample verbatim, without announcing it.
            pending = if eof { used } else { used & !1 };
        }
        if fds[1].revents & POLLOUT != 0 {
            if pending >= 2 {
                if let Some(on) = activity.pcm(&bytes[..pending], Instant::now()) {
                    notice.send(on);
                }
            }
            output.write_all(&bytes[..pending])?;
            bytes.copy_within(pending..used, 0);
            used -= pending;
            pending = 0;
        }
    }
}

fn play(program: &Path, endpoint: &Path, args: &[String], format: Format) -> io::Result<i32> {
    let mut cmd = Command::new(program);
    cmd.args(args);
    let mut child = spawn_player(&mut cmd, process::id())?;
    let mut output = child.stdin.take().expect("piped player stdin");
    // Minimise additional buffering/backpressure changes in front of ALSA.
    // F_GETPIPE_SZ reports actual capacity when shrinking is unavailable.
    let pipe_bytes = unsafe {
        fcntl(output.as_raw_fd(), 1031, 4096 as c_int); // F_SETPIPE_SZ
        fcntl(output.as_raw_fd(), 1032) // F_GETPIPE_SZ
    };
    let pipe_bytes = if pipe_bytes > 0 {
        pipe_bytes as usize
    } else {
        65536
    };
    let mut activity = Activity::new(format, pipe_bytes, Instant::now());
    let mut notice = Notice::new(endpoint);
    // Use the fd directly: a buffered Stdin can retain bytes invisible to poll.
    let mut input = unsafe { fs::File::from_raw_fd(0) };
    let result = relay(&mut input, &mut output, &mut activity, &mut notice);
    drop(output);
    if result.is_err() {
        let _ = child.kill();
    }
    let status = child.wait();
    if activity.audible_until.is_some() {
        notice.send(false);
    }
    result?;
    let status = status?;
    Ok(status
        .code()
        .unwrap_or_else(|| 128 + status.signal().unwrap_or(1)))
}

pub fn run() -> io::Result<i32> {
    let program = ["/usr/bin/aplay", "/bin/aplay"]
        .iter()
        .map(Path::new)
        .find(|p| p.is_file())
        .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;
    let args: Vec<_> = env::args_os().skip(1).collect();
    let strings: Option<Vec<_>> = args.iter().map(|a| a.to_str().map(str::to_owned)).collect();
    if let Some((args, format)) = strings
        .as_ref()
        .and_then(|s| Format::from_args(s).map(|f| (s, f)))
    {
        play(program, Path::new(ENDPOINT), args, format)
    } else {
        // File playback and other CLI callers retain the system player's semantics.
        Err(Command::new(program).args(args).exec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        os::{fd::OwnedFd, unix::net::UnixStream},
        thread,
        time::Duration,
    };

    #[test]
    fn fragmented_input_idle_tail_and_missing_owner_preserve_bytes() {
        let path = env::temp_dir().join(format!("aplay-relay-{}.sock", process::id()));
        let owner = UnixDatagram::bind(&path).unwrap();
        let (mut writer, input) = UnixStream::pair().unwrap();
        let mut player = spawn_player(
            Command::new("/bin/cat").stdout(Stdio::piped()),
            process::id(),
        )
        .unwrap();
        let mut output = player.stdin.take().unwrap();
        let destination = path.clone();
        let worker = thread::spawn(move || {
            let mut input = fs::File::from(OwnedFd::from(input));
            let format = Format::from_args(&crate::tests::args()).unwrap();
            let mut activity = Activity::new(format, 4096, Instant::now());
            relay(
                &mut input,
                &mut output,
                &mut activity,
                &mut Notice::new(&destination),
            )
            .unwrap();
        });
        let mut packet = [0; 96];
        owner
            .set_read_timeout(Some(Duration::from_millis(40)))
            .unwrap();
        writer.write_all(&[0, 0, 8]).unwrap();
        assert!(matches!(
            owner.recv(&mut packet).unwrap_err().kind(),
            io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
        ));
        owner
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        writer.write_all(&[9]).unwrap();
        let prefix = format!(
            "TTS1 {} {} ",
            process::id(),
            start_ticks(&fs::read_to_string("/proc/self/stat").unwrap()).unwrap()
        );
        for state in ["1 ON\n", "2 OFF\n"] {
            let size = owner.recv(&mut packet).unwrap();
            assert_eq!(&packet[..size], format!("{prefix}{state}").as_bytes());
        }
        fs::remove_file(path).unwrap();
        drop(owner);
        writer.write_all(&[10, 11, 12]).unwrap();
        drop(writer);
        worker.join().unwrap();
        let result = player.wait_with_output().unwrap();
        assert!(result.status.success());
        assert_eq!(result.stdout, [0, 0, 8, 9, 10, 11, 12]);
    }

    #[test]
    #[ignore = "standalone integration fixture with fake player and isolated socket"]
    fn run_fixture() {
        let args = crate::tests::args();
        let format = Format::from_args(&args).unwrap();
        let program = env::var_os("TEST_APLAY_PROGRAM").unwrap();
        let endpoint = env::var_os("TEST_TTS_SOCKET").unwrap();
        assert_eq!(
            play(Path::new(&program), Path::new(&endpoint), &args, format).unwrap(),
            0
        );
    }

    #[test]
    fn detects_fork_registration_race() {
        assert_eq!(
            spawn_player(&mut Command::new("/bin/true"), 0)
                .unwrap_err()
                .raw_os_error(),
            Some(3)
        );
    }

    #[test]
    #[ignore = "subprocess helper for player_dies_with_wrapper"]
    fn parent_death_helper() {
        let mut child = spawn_player(Command::new("/bin/sleep").arg("30"), process::id()).unwrap();
        let mut listener =
            UnixStream::connect(env::var_os("TEST_CHILD_PID_SOCKET").unwrap()).unwrap();
        listener.write_all(&child.id().to_ne_bytes()).unwrap();
        child.wait().unwrap();
    }

    #[test]
    fn player_dies_with_wrapper() {
        use std::os::unix::net::UnixListener;
        let socket_path = env::temp_dir().join(format!("aplay-pid-{}.sock", process::id()));
        let listener = UnixListener::bind(&socket_path).unwrap();
        let mut wrapper = Command::new(env::current_exe().unwrap())
            .args(["--ignored", "--exact", "linux::tests::parent_death_helper"])
            .env("TEST_CHILD_PID_SOCKET", &socket_path)
            .spawn()
            .unwrap();
        listener.set_nonblocking(true).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut conn = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock && Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(10))
                }
                Err(e) => {
                    let _ = wrapper.kill();
                    let _ = wrapper.wait();
                    let _ = fs::remove_file(socket_path);
                    panic!("waiting for wrapper child: {e}");
                }
            }
        };
        conn.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
        let mut pid = [0; 4];
        conn.read_exact(&mut pid).unwrap();
        fs::remove_file(socket_path).unwrap();
        wrapper.kill().unwrap();
        wrapper.wait().unwrap();
        let path = format!("/proc/{}/stat", u32::from_ne_bytes(pid));
        for _ in 0..100 {
            match fs::read_to_string(&path) {
                Err(e) if e.kind() == io::ErrorKind::NotFound => return,
                Ok(s) if s.rsplit_once(')').unwrap().1.trim_start().starts_with('Z') => return,
                _ => thread::sleep(Duration::from_millis(10)),
            }
        }
        panic!("player survived its wrapper");
    }
}
