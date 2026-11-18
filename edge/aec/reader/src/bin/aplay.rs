// SPDX-License-Identifier: MIT
//! Private PATH adapter for the client's raw PCM player.
use std::time::{Duration, Instant};

const HEARTBEAT: Duration = Duration::from_millis(500);
const TAIL_GUARD: Duration = Duration::from_millis(60);

#[derive(Clone, Copy)]
struct Format {
    bytes_per_second: u64,
    buffer: Duration,
    period: Duration,
}

impl Format {
    fn from_args(args: &[String]) -> Option<Self> {
        let args: Vec<_> = args.iter().map(String::as_str).collect();
        let ["--quiet", "-t", "raw", "-f", "S16_LE", "-r", rate, "-c", "1", "--buffer-size", buffer, "--period-size", period, "-"] =
            args.as_slice()
        else {
            return None;
        };
        let rate: u32 = rate.parse().ok()?;
        let buffer: u32 = buffer.parse().ok()?;
        let period: u32 = period.parse().ok()?;
        if rate == 0 || period == 0 || period > buffer || buffer > rate {
            return None;
        }
        Some(Self {
            bytes_per_second: u64::from(rate) * 2,
            buffer: Duration::from_nanos(u64::from(buffer) * 1_000_000_000 / u64::from(rate)),
            period: Duration::from_nanos(u64::from(period) * 1_000_000_000 / u64::from(rate)),
        })
    }

    fn byte_time(self, bytes: usize) -> Duration {
        Duration::from_nanos(bytes as u64 * 1_000_000_000 / self.bytes_per_second)
    }
}

struct Activity {
    format: Format,
    queued_until: Instant,
    max_queue: Duration,
    audible_until: Option<Instant>,
    heartbeat: Instant,
}

impl Activity {
    fn new(format: Format, pipe_bytes: usize, now: Instant) -> Self {
        Self {
            format,
            queued_until: now,
            max_queue: format.byte_time(pipe_bytes) + format.buffer + format.period,
            audible_until: None,
            heartbeat: now,
        }
    }

    // Called immediately before the bounded, atomic write to the ready pipe.
    // In S16_LE silence both bytes are zero, including across split reads.
    fn pcm(&mut self, bytes: &[u8], now: Instant) -> Option<bool> {
        self.queued_until = (self.queued_until.max(now) + self.format.byte_time(bytes.len()))
            .min(now + self.max_queue);
        if let Some(last) = bytes.iter().rposition(|&b| b != 0) {
            let trailing = self.format.byte_time(bytes.len() - 1 - last);
            let end = self.queued_until - trailing + self.format.buffer + TAIL_GUARD;
            if self.audible_until.replace(end).is_none() {
                self.heartbeat = now + HEARTBEAT;
                return Some(true);
            }
        }
        None
    }

    fn deadline(&self) -> Option<Instant> {
        self.audible_until.map(|end| end.min(self.heartbeat))
    }

    fn tick(&mut self, now: Instant) -> Option<bool> {
        let end = self.audible_until?;
        if now >= end {
            self.audible_until = None;
            return Some(false);
        }
        if now >= self.heartbeat {
            self.heartbeat = now + HEARTBEAT;
            return Some(true);
        }
        None
    }
}

#[cfg(target_os = "linux")]
#[path = "../playback_linux.rs"]
mod linux;

#[cfg(target_os = "linux")]
fn main() {
    match linux::run() {
        Ok(status) => std::process::exit(status),
        Err(error) => {
            eprintln!("aplay adapter: {error}");
            std::process::exit(1);
        }
    }
}

#[cfg(all(not(target_os = "linux"), not(test)))]
fn main() {
    eprintln!("the private OH2P player requires Linux");
    std::process::exit(1);
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn args() -> Vec<String> {
        "--quiet -t raw -f S16_LE -r 24000 -c 1 --buffer-size 1440 --period-size 360 -"
            .split_whitespace()
            .map(str::to_owned)
            .collect()
    }

    #[test]
    fn supported_client_formats() {
        let mut args = args();
        assert_eq!(
            Format::from_args(&args).unwrap().buffer,
            Duration::from_millis(60)
        );
        args[6] = "16000".into();
        args[10] = "480".into();
        args[12] = "160".into();
        assert_eq!(
            Format::from_args(&args).unwrap().period,
            Duration::from_millis(10)
        );
        args[13] = "some.wav".into();
        assert!(Format::from_args(&args).is_none());
    }

    #[test]
    fn idle_and_silent_pipe_then_two_utterances_and_heartbeat() {
        let now = Instant::now();
        let mut a = Activity::new(Format::from_args(&args()).unwrap(), 4096, now);
        let at = |ms| now + Duration::from_millis(ms);
        assert_eq!(a.deadline(), None);
        assert_eq!(a.pcm(&[0; 960], now), None);
        assert_eq!(a.pcm(&[1; 960], at(20)), Some(true));
        for ms in (40..=520).step_by(20) {
            assert_eq!(a.pcm(&[1; 960], at(ms)), None);
        }
        assert_eq!(a.tick(at(520)), Some(true));
        for ms in (540..660).step_by(20) {
            assert_eq!(a.pcm(&[0; 960], at(ms)), None);
            assert_eq!(a.tick(at(ms)), None);
        }
        assert_eq!(a.tick(at(660)), Some(false));
        assert_eq!(a.pcm(&[1, 0], at(1000)), Some(true));
        assert_eq!(a.tick(at(1200)), Some(false));
    }

    #[test]
    fn tail_accounts_for_queued_audio_but_is_bounded() {
        let now = Instant::now();
        let mut a = Activity::new(Format::from_args(&args()).unwrap(), 4096, now);
        for _ in 0..1000 {
            a.pcm(&[1; 2048], now);
        }
        assert_eq!(a.tick(now + Duration::from_millis(180)), None);
        assert_eq!(a.tick(now + Duration::from_millis(300)), Some(false));
    }
}
