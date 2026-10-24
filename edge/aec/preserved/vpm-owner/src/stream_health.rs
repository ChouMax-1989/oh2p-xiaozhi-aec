//! Detect loss of enhanced callbacks, not quiet audio. Called only as raw frames arrive.
use std::time::{Duration, Instant};
const STALL: Duration = Duration::from_secs(5);
const RETRY: Duration = Duration::from_secs(30);
pub struct StreamHealth {
    seen: u64,
    last_progress: Instant,
    retry_after: Option<Instant>,
}
impl StreamHealth {
    pub fn new(now: Instant, count: u64) -> Self {
        Self { seen: count, last_progress: now, retry_after: None }
    }
    pub fn needs_rearm(&mut self, now: Instant, count: u64, enabled: bool) -> bool {
        if !enabled || count != self.seen {
            self.seen = count;
            self.last_progress = now;
            self.retry_after = None;
            return false;
        }
        if now.duration_since(self.last_progress) < STALL ||
            self.retry_after.is_some_and(|until| now < until) { return false; }
        self.retry_after = Some(now + RETRY);
        true
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn stalled_callbacks_rearm_with_bounded_retry() {
        let t = Instant::now(); let mut h = StreamHealth::new(t, 10);
        assert!(!h.needs_rearm(t + Duration::from_secs(4),10,true));
        assert!(h.needs_rearm(t + Duration::from_secs(5),10,true));
        assert!(!h.needs_rearm(t + Duration::from_secs(34),10,true));
        assert!(h.needs_rearm(t + Duration::from_secs(35),10,true));
    }
    #[test] fn healthy_silence_does_not_rearm() {
        let t = Instant::now(); let mut h = StreamHealth::new(t, 0);
        for i in 1..3600 { assert!(!h.needs_rearm(t + Duration::from_secs(i),i,true)); }
    }
    #[test] fn recovery_resets_timer_and_explicit_capture_disables_guard() {
        let t = Instant::now(); let mut h = StreamHealth::new(t, 0);
        assert!(h.needs_rearm(t + Duration::from_secs(5),0,true));
        assert!(!h.needs_rearm(t + Duration::from_secs(6),1,true));
        assert!(h.needs_rearm(t + Duration::from_secs(11),1,true));
        assert!(!h.needs_rearm(t + Duration::from_secs(60),1,false));
        assert!(!h.needs_rearm(t + Duration::from_secs(64),1,true));
        assert!(h.needs_rearm(t + Duration::from_secs(65),1,true));
    }
}
