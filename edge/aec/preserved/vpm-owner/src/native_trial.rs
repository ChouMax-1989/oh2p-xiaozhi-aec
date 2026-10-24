//! Software-only native VPM enhanced callback trial; never an AEC acceptance claim.
use std::fs::{self, File, Permissions};
use std::io::{self, Read};
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::net::UnixDatagram;
use std::time::{Duration, Instant};

pub const PATH: &str = "/tmp/family-voice-vpm-native-trial.sock";
const LEASE: Duration = Duration::from_secs(2);
pub struct NativeTrial {
    socket: UnixDatagram,
    boot: String,
    enabled_until: Option<Instant>,
}
use crate::native_trial_protocol::parse;
impl NativeTrial {
    pub fn bind() -> io::Result<Self> {
        // Called only after the existing exclusive owner lock is held.
        if let Ok(meta) = fs::symlink_metadata(PATH) {
            if !meta.file_type().is_socket() || meta.uid() != 0 { return Err(io::Error::other("unsafe native trial endpoint")); }
            fs::remove_file(PATH)?;
        }
        let mut entropy = [0u8; 16];
        File::open("/dev/urandom")?.read_exact(&mut entropy)?;
        let boot = entropy.iter().map(|b| format!("{b:02x}")).collect();
        let socket = UnixDatagram::bind(PATH)?;
        fs::set_permissions(PATH, Permissions::from_mode(0o600))?;
        socket.set_nonblocking(true)?;
        Ok(Self { socket, boot, enabled_until: None })
    }
    pub fn enabled(&self, now: Instant) -> bool { self.enabled_until.is_some_and(|until| now < until) }
    pub fn poll(&mut self, now: Instant) {
        // Bounded control work, no thread, raw PCM or persistent config.
        for _ in 0..8 {
            let mut buffer = [0u8; 128];
            let Ok((size, peer)) = self.socket.recv_from(&mut buffer) else { break; };
            let Some(path) = peer.as_pathname() else { continue; };
            let Ok(meta) = fs::symlink_metadata(path) else { continue; };
            if !meta.file_type().is_socket() || meta.uid() != 0 || meta.permissions().mode() & 0o777 != 0o600 { continue; }
            let Some((enabled, revision)) = parse(&buffer[..size]) else { continue; };
            self.enabled_until = if enabled { Some(now + LEASE) } else { None };
            let response = format!("FVNT1 OK {} {} {}\n", u8::from(enabled), revision, self.boot);
            let _ = self.socket.send_to(response.as_bytes(), path);
        }
    }
}
impl Drop for NativeTrial { fn drop(&mut self) { let _ = fs::remove_file(PATH); } }
