// SPDX-License-Identifier: MIT
//! Private PATH player for the client's raw PCM, with playback-state notices.
use std::time::{Duration, Instant};

const HEARTBEAT: Duration = Duration::from_millis(500);
const TAIL_GUARD: Duration = Duration::from_millis(60);

#[derive(Clone, Copy)]
struct Format {
    rate: u32,
    buffer: u32,
    period: u32,
}

impl Format {
    fn from_args(args: &[String]) -> Option<Self> {
        let args: Vec<_> = args.iter().map(String::as_str).collect();
        let ["--quiet", "-t", "raw", "-f", "S16_LE", "-r", rate, "-c", "1", "--buffer-size", buffer, "--period-size", period, "-"] =
            args.as_slice()
        else {
            return None;
        };
        let rate = rate.parse().ok()?;
        let buffer = buffer.parse().ok()?;
        let period = period.parse().ok()?;
        if rate == 0 || rate > 192_000 || period == 0 || period >= buffer || buffer > rate {
            return None;
        }
        Some(Self {
            rate,
            buffer,
            period,
        })
    }
}

#[derive(Default)]
struct Activity {
    // Number of accepted silent frames following the last audible frame. An
    // ALSA delay at or below this count means that only silence can remain.
    trailing_silence: Option<usize>,
    heartbeat: Option<Instant>,
    quiet_until: Option<Instant>,
}

fn has_sound(bytes: &[u8]) -> bool {
    bytes.chunks_exact(2).any(|sample| sample != [0, 0])
}

impl Activity {
    fn active(&self) -> bool {
        self.heartbeat.is_some()
    }

    fn queued_sound(&self) -> bool {
        self.trailing_silence.is_some()
    }

    fn before_write(&mut self, audible: bool, now: Instant) -> bool {
        if audible {
            self.quiet_until = None;
            if !self.active() {
                self.heartbeat = Some(now + HEARTBEAT);
                return true;
            }
        }
        false
    }

    fn accepted(&mut self, bytes: &[u8]) {
        let mut frames = bytes.chunks_exact(2);
        let count = frames.len();
        if let Some(index) = frames.rposition(|sample| sample != [0, 0]) {
            self.trailing_silence = Some(count - index - 1);
        } else if let Some(trailing) = &mut self.trailing_silence {
            *trailing = trailing.saturating_add(count);
        }
    }

    fn observe_delay(&mut self, frames: usize) {
        if self
            .trailing_silence
            .is_some_and(|trailing| frames <= trailing)
        {
            self.queue_ended();
        }
    }

    fn queue_ended(&mut self) {
        self.trailing_silence = None;
    }

    fn tick(&mut self, pending_sound: bool, now: Instant) -> Option<bool> {
        let heartbeat = self.heartbeat?;
        if pending_sound || self.queued_sound() {
            self.quiet_until = None;
        } else if now >= *self.quiet_until.get_or_insert(now + TAIL_GUARD) {
            self.heartbeat = None;
            self.quiet_until = None;
            return Some(false);
        }
        if now >= heartbeat {
            self.heartbeat = Some(now + HEARTBEAT);
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
    if let Err(error) = linux::run() {
        eprintln!("aplay adapter: {error}");
        std::process::exit(1);
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

    #[test]
    fn supported_client_formats() {
        let mut args: Vec<_> =
            "--quiet -t raw -f S16_LE -r 24000 -c 1 --buffer-size 1440 --period-size 360 -"
                .split_whitespace()
                .map(str::to_owned)
                .collect();
        let format = Format::from_args(&args).unwrap();
        assert_eq!(
            (format.rate, format.buffer, format.period),
            (24000, 1440, 360)
        );
        args[6] = "16000".into();
        args[10] = "480".into();
        args[12] = "160".into();
        assert_eq!(Format::from_args(&args).unwrap().period, 160);
        args[13] = "some.wav".into();
        assert!(Format::from_args(&args).is_none());
    }

    #[test]
    fn delayed_playback_holds_on_until_measured_completion() {
        let now = Instant::now();
        let at = |ms| now + Duration::from_millis(ms);
        let mut activity = Activity::default();
        assert!(activity.before_write(true, now));
        assert_eq!(activity.tick(true, at(140)), None); // Write still pending.
        activity.accepted(&[1; 960]);
        assert_eq!(activity.tick(false, at(200)), None); // Delay unavailable.
        activity.observe_delay(480);
        assert_eq!(activity.tick(false, at(500)), Some(true));
        activity.observe_delay(0);
        assert_eq!(activity.tick(false, at(550)), None);
        assert_eq!(activity.tick(false, at(609)), None);
        assert_eq!(activity.tick(false, at(610)), Some(false));
    }

    #[test]
    fn queued_silence_does_not_keep_playback_active() {
        let now = Instant::now();
        let mut activity = Activity::default();
        assert!(!has_sound(&[0, 0, 1]));
        assert!(has_sound(&[0, 1]));
        assert!(!activity.before_write(false, now));
        assert!(activity.before_write(true, now));
        activity.accepted(&[0, 1, 0, 0]);
        activity.accepted(&[0; 960]);
        activity.observe_delay(482);
        assert!(activity.queued_sound());
        activity.observe_delay(481);
        assert!(!activity.queued_sound());
        assert_eq!(activity.tick(false, now), None);
        assert_eq!(activity.tick(false, now + TAIL_GUARD), Some(false));
        assert!(activity.before_write(true, now + TAIL_GUARD));
    }

    #[test]
    fn xrun_drops_old_queue_without_dropping_unwritten_samples() {
        let now = Instant::now();
        let mut activity = Activity::default();
        assert!(activity.before_write(true, now));
        activity.accepted(&[1, 0]);
        activity.queue_ended();
        assert_eq!(activity.tick(true, now + HEARTBEAT), Some(true));
        activity.accepted(&[2, 0]);
        activity.observe_delay(1);
        assert!(activity.queued_sound());
    }
}
