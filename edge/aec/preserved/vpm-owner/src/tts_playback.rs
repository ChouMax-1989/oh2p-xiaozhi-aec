// SPDX-License-Identifier: LGPL-3.0-or-later
//! Local wrapper reports its PID, /proc start time and a per-process sequence.
//! The VPM switch follows the union of all live playback intervals.

use std::collections::HashMap;
use std::time::{Duration, Instant};

const LEASE: Duration = Duration::from_secs(3);
const MAX_SENDERS: usize = 32;
const MAX_PACKET: usize = 80;

#[derive(Debug, PartialEq)]
struct Notice {
    pid: u32,
    start: u64,
    seq: u64,
    on: bool,
}

fn parse(packet: &[u8]) -> Option<Notice> {
    if packet.len() > MAX_PACKET {
        return None;
    }
    let mut fields = std::str::from_utf8(packet.strip_suffix(b"\n")?)
        .ok()?
        .split(' ');
    if fields.next()? != "TTS1" {
        return None;
    }
    let mut number = || {
        let word = fields.next()?;
        if word.is_empty() || !word.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        word.parse::<u64>().ok()
    };
    let pid = u32::try_from(number()?).ok()?;
    let start = number()?;
    let seq = number()?;
    let on = match fields.next()? {
        "ON" => true,
        "OFF" => false,
        _ => return None,
    };
    if pid == 0 || pid > i32::MAX as u32 || seq == 0 || fields.next().is_some() {
        return None;
    }
    Some(Notice {
        pid,
        start,
        seq,
        on,
    })
}

// comm (field 2) can contain spaces, parentheses and non-UTF8 bytes.
fn live_start(stat: &[u8]) -> Option<u64> {
    let tail = &stat[stat.iter().rposition(|&b| b == b')')? + 1..];
    let mut fields = tail
        .split(|b| b.is_ascii_whitespace())
        .filter(|p| !p.is_empty());
    match fields.next()? {
        b"Z" | b"X" | b"x" => None,
        _ => std::str::from_utf8(fields.nth(18)?).ok()?.parse().ok(),
    }
}

struct Sender {
    start: u64,
    last_seq: u64,
    active_until: Option<Instant>,
}

#[derive(Default)]
struct Playback {
    senders: HashMap<u32, Sender>,
}

impl Playback {
    fn active(&self) -> bool {
        self.senders.values().any(|s| s.active_until.is_some())
    }

    fn accept(&mut self, notice: Notice, now: Instant, process_start: Option<u64>) {
        if process_start != Some(notice.start) {
            return;
        }
        if !self.senders.contains_key(&notice.pid) && self.senders.len() >= MAX_SENDERS {
            return;
        }
        let sender = self.senders.entry(notice.pid).or_insert(Sender {
            start: notice.start,
            last_seq: 0,
            active_until: None,
        });
        if sender.start == notice.start && notice.seq <= sender.last_seq {
            return;
        }
        sender.start = notice.start;
        sender.last_seq = notice.seq;
        sender.active_until = notice.on.then(|| now + LEASE);
    }

    fn reap(&mut self, now: Instant, mut process_start: impl FnMut(u32) -> Option<u64>) {
        self.senders.retain(|pid, sender| {
            if sender.active_until.is_some_and(|end| now >= end) {
                sender.active_until = None;
            }
            // Keep the sequence after OFF/expiry while the process lives.
            process_start(*pid) == Some(sender.start)
        });
    }
}

#[cfg(unix)]
pub mod receiver {
    use super::*;
    use std::ffi::c_int;
    use std::fs;
    use std::io;
    use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
    use std::os::unix::net::UnixDatagram;
    use std::path::{Path, PathBuf};

    pub const SOCKET_PATH: &str = "/tmp/open-xiaoai/tts-playback.sock";
    pub type SetExternalState = unsafe extern "C" fn(c_int, ...) -> c_int;
    const TTS_START: c_int = 6;
    const TTS_END: c_int = 7;
    const REAP_INTERVAL: Duration = Duration::from_millis(250);

    unsafe extern "C" {
        fn geteuid() -> u32;
    }

    fn process_start(pid: u32) -> Option<u64> {
        live_start(&fs::read(format!("/proc/{pid}/stat")).ok()?)
    }

    pub struct Receiver {
        socket: UnixDatagram,
        path: PathBuf,
        inode: (u64, u64),
        playback: Playback,
        reap_at: Instant,
        set_external_state: Box<dyn FnMut(c_int) -> c_int>,
        reported: bool,
    }

    impl Receiver {
        // The event-file lock in run() serializes owners, including stale socket
        // recovery after SIGKILL. /tmp is sticky; require our own private child.
        pub fn bind(
            path: &Path,
            set_external_state: impl FnMut(c_int) -> c_int + 'static,
        ) -> io::Result<Self> {
            let uid = unsafe { geteuid() };
            let parent = path
                .parent()
                .ok_or_else(|| io::Error::other("missing socket parent"))?;
            let dir = fs::symlink_metadata(parent)?;
            if !dir.is_dir() || dir.uid() != uid || dir.mode() & 0o777 != 0o700 {
                return Err(io::Error::other("unsafe TTS socket directory"));
            }
            match fs::symlink_metadata(path) {
                Ok(m) if m.file_type().is_socket() && m.uid() == uid => fs::remove_file(path)?,
                Ok(_) => return Err(io::Error::other("unsafe existing TTS socket path")),
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
            let socket = UnixDatagram::bind(path)?;
            let meta = fs::symlink_metadata(path)?;
            let receiver = Self {
                socket,
                path: path.to_owned(),
                inode: (meta.dev(), meta.ino()),
                playback: Playback::default(),
                reap_at: Instant::now(),
                set_external_state: Box::new(set_external_state),
                reported: false,
            };
            fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
            receiver.socket.set_nonblocking(true)?;
            Ok(receiver)
        }

        fn report(&mut self, active: bool) {
            if active != self.reported {
                let code = if active { TTS_START } else { TTS_END };
                let raw = (self.set_external_state)(code);
                eprintln!("vpm-owner: vpm_set_external_state({code}) returned {raw}");
                self.reported = active;
            }
        }

        pub fn poll(&mut self) {
            let now = Instant::now();
            // Reject truncated datagrams and bound work per audio iteration.
            let mut packet = [0u8; MAX_PACKET + 1];
            for _ in 0..8 {
                let Ok(size) = self.socket.recv(&mut packet) else {
                    break;
                };
                if let Some(notice) = parse(&packet[..size]) {
                    let start = process_start(notice.pid);
                    self.playback.accept(notice, now, start);
                    self.report(self.playback.active());
                }
            }
            if now >= self.reap_at {
                self.playback.reap(now, process_start);
                self.report(self.playback.active());
                self.reap_at = now + REAP_INTERVAL;
            }
        }
    }

    impl Drop for Receiver {
        fn drop(&mut self) {
            self.report(false);
            if let Ok(meta) = fs::symlink_metadata(&self.path) {
                if meta.file_type().is_socket() && (meta.dev(), meta.ino()) == self.inode {
                    let _ = fs::remove_file(&self.path);
                }
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::os::unix::fs::{symlink, DirBuilderExt};
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::{cell::RefCell, rc::Rc};

        struct Dir(PathBuf);

        impl Dir {
            fn new() -> Self {
                static NEXT: AtomicUsize = AtomicUsize::new(0);
                let path = std::env::temp_dir().join(format!(
                    "vpm-tts-{}-{}",
                    std::process::id(),
                    NEXT.fetch_add(1, Ordering::Relaxed)
                ));
                fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
                Self(path)
            }
        }

        impl Drop for Dir {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }

        #[test]
        fn socket_reports_vendor_edges_once_and_recovers_after_owner_restart() {
            let dir = Dir::new();
            let path = dir.0.join("tts.sock");
            let calls = Rc::new(RefCell::new(Vec::new()));
            let bind = || {
                let calls = calls.clone();
                Receiver::bind(&path, move |code| {
                    calls.borrow_mut().push(code);
                    19 // Raw firmware return value doesn't drive retries.
                })
                .unwrap()
            };
            let mut receiver = bind();
            let sender = UnixDatagram::unbound().unwrap();
            let pid = std::process::id();
            let start = process_start(pid).unwrap();
            let send = |seq, state| {
                sender
                    .send_to(
                        format!("TTS1 {pid} {start} {seq} {state}\n").as_bytes(),
                        &path,
                    )
                    .unwrap();
            };
            send(1, "ON");
            send(2, "ON");
            send(4, "OFF");
            send(3, "ON");
            send(5, "ON");
            send(4, "OFF");
            receiver.poll();
            assert_eq!(*calls.borrow(), vec![6, 7, 6]);
            drop(receiver);
            assert!(!path.exists());
            // A still-running player refreshes with the next heartbeat.
            let mut receiver = bind();
            send(6, "ON");
            receiver.poll();
            send(7, "OFF");
            receiver.poll();
            drop(receiver);
            assert_eq!(*calls.borrow(), vec![6, 7, 6, 7, 6, 7]);
        }

        #[test]
        fn socket_detects_killed_player_without_off() {
            let dir = Dir::new();
            let path = dir.0.join("tts.sock");
            let calls = Rc::new(RefCell::new(Vec::new()));
            let record = calls.clone();
            let mut receiver = Receiver::bind(&path, move |code| {
                record.borrow_mut().push(code);
                0
            })
            .unwrap();
            let mut child = std::process::Command::new("sleep")
                .arg("3")
                .spawn()
                .unwrap();
            let pid = child.id();
            let start = process_start(pid).unwrap();
            UnixDatagram::unbound()
                .unwrap()
                .send_to(format!("TTS1 {pid} {start} 1 ON\n").as_bytes(), &path)
                .unwrap();
            receiver.poll();
            child.kill().unwrap();
            child.wait().unwrap();
            receiver.reap_at = Instant::now();
            receiver.poll();
            assert_eq!(*calls.borrow(), vec![6, 7]);
        }

        #[test]
        fn bind_rejects_symlinks_public_directories_and_regular_files() {
            let dir = Dir::new();
            let path = dir.0.join("tts.sock");
            fs::write(&path, b"keep").unwrap();
            assert!(Receiver::bind(&path, |_| 0).is_err());
            assert_eq!(fs::read(&path).unwrap(), b"keep");
            fs::remove_file(&path).unwrap();
            symlink(dir.0.join("missing"), &path).unwrap();
            assert!(Receiver::bind(&path, |_| 0).is_err());
            assert!(path.is_symlink());
            fs::remove_file(&path).unwrap();
            fs::set_permissions(&dir.0, fs::Permissions::from_mode(0o755)).unwrap();
            assert!(Receiver::bind(&path, |_| 0).is_err());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_malformed_and_truncated_packets() {
        let ok = b"TTS1 13 12 18446744073709551615 ON\n";
        assert_eq!(
            parse(ok),
            Some(Notice {
                pid: 13,
                start: 12,
                seq: u64::MAX,
                on: true
            })
        );
        for packet in [
            &b"TTS1 13 12 1 ON"[..],
            b"TTS1 13 12 1 ON\n\0",
            b"TTS1 13 12 1 ON\r\n",
            b"TTS1 13 12 1  ON\n",
            b"TTS1 +13 12 1 ON\n",
            b"TTS1 0 12 1 ON\n",
            b"TTS1 13 12 0 OFF\n",
            b"TTS1 13 12 18446744073709551616 ON\n",
            b"TTS1 13 12 1 OFF\nmore\n",
            &[b'x'; MAX_PACKET + 1],
        ] {
            assert!(parse(packet).is_none());
        }
    }

    #[test]
    fn proc_stat_tolerates_comm_bytes_and_rejects_zombies() {
        let stat = b"43 (x) \xff y)) S 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 876 99";
        assert_eq!(live_start(stat), Some(876));
        let mut zombie = stat.to_vec();
        *zombie.iter_mut().find(|b| **b == b'S').unwrap() = b'Z';
        assert_eq!(live_start(&zombie), None);
        assert_eq!(live_start(b""), None);
    }

    #[test]
    fn preserves_other_active_players_and_rejects_reused_pid_and_stale_sequences() {
        let mut playback = Playback::default();
        let now = Instant::now();
        let mut accept = |pid, start, seq, on, live| {
            playback.accept(
                Notice {
                    pid,
                    start,
                    seq,
                    on,
                },
                now,
                live,
            );
            playback.active()
        };
        assert!(!accept(1, 10, 1, true, None)); // Exited before delivery.
        assert!(accept(1, 10, 2, true, Some(10)));
        assert!(accept(2, 20, 1, true, Some(20)));
        assert!(accept(1, 10, 3, false, Some(10)));
        assert!(!accept(2, 20, 2, false, Some(20)));
        assert!(!accept(1, 10, 2, true, Some(10))); // Late heartbeat after OFF.
        assert!(accept(1, 99, 1, true, Some(99))); // PID reused.
        assert!(accept(1, 10, 4, false, Some(99))); // Old OFF.
        assert!(!accept(1, 99, 2, false, Some(99)));
    }

    #[test]
    fn lost_off_expires_and_only_a_fresh_heartbeat_rearms() {
        let mut playback = Playback::default();
        let now = Instant::now();
        let notice = |seq| Notice {
            pid: 1,
            start: 10,
            seq,
            on: true,
        };
        playback.accept(notice(1), now, Some(10));
        playback.accept(notice(2), now + Duration::from_secs(1), Some(10));
        playback.reap(now + LEASE, |_| Some(10));
        assert!(playback.active());
        let expired = now + LEASE + Duration::from_secs(1);
        playback.reap(expired, |_| Some(10));
        assert!(!playback.active());
        playback.accept(notice(2), expired, Some(10));
        assert!(!playback.active());
        playback.accept(notice(3), expired, Some(10));
        assert!(playback.active());
        playback.reap(expired, |_| None);
        assert!(!playback.active());
        assert!(playback.senders.is_empty());
    }
}
