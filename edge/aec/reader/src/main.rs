// SPDX-License-Identifier: MIT
// FVSH1 framing from oh2p-family-voice-firmware-source 2ab6b4e, shared-audio patch.
// Only Unix sockets and stdout are used; raw audio is never written to a file.
use std::{env, fs, io::{self, Write}, os::unix::{fs::{FileTypeExt, MetadataExt, PermissionsExt}, net::UnixDatagram}, path::{Path, PathBuf}, time::{Duration, SystemTime, UNIX_EPOCH}};
const SERVER: &str = "/tmp/family-voice-vpm-audio.sock";
const MAGIC: &[u8] = b"FVSH1 PCM\n";
const MAX: usize = 16_384;
fn invalid(message: &str) -> io::Error { io::Error::new(io::ErrorKind::InvalidData, message) }
fn validate_args(args: &[String]) -> io::Result<()> {
    let expected = ["--quiet", "-t", "raw", "-D", "noop", "-f", "S16_LE", "-r", "16000", "-c", "1", "--buffer-size", "1440", "--period-size", "360"];
    if args.iter().map(String::as_str).eq(expected) { Ok(()) } else { Err(invalid("FVSH reader accepts only deployed 16kHz mono S16_LE request")) }
}
fn payload(packet: &[u8]) -> io::Result<&[u8]> {
    let header = MAGIC.len() + 4;
    if packet.len() < header || &packet[..MAGIC.len()] != MAGIC { return Err(invalid("invalid FVSH1 header")); }
    let size = u32::from_le_bytes(packet[MAGIC.len()..header].try_into().unwrap()) as usize;
    if size == 0 || size > MAX || size % 2 != 0 || packet.len() != header + size { return Err(invalid("invalid FVSH1 length")); }
    Ok(&packet[header..])
}
struct Reader { socket: UnixDatagram, local: PathBuf }
impl Reader {
    fn connect(server: &Path, directory: &Path) -> io::Result<Self> {
        let meta = fs::symlink_metadata(server)?;
        if !meta.file_type().is_socket() || meta.uid() != 0 || meta.permissions().mode() & 0o777 != 0o600 { return Err(invalid("unsafe owner socket")); }
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let local = directory.join(format!("fvsh-{}-{nonce}.sock", std::process::id()));
        let socket = UnixDatagram::bind(&local)?;
        let reader = Self { socket, local };
        fs::set_permissions(&reader.local, fs::Permissions::from_mode(0o600))?;
        reader.socket.connect(server)?;
        reader.socket.set_read_timeout(Some(Duration::from_millis(500)))?;
        reader.socket.send(b"FVSH1 SUB\n")?;
        let mut ack = [0; 64];
        let n = reader.socket.recv(&mut ack)?;
        if &ack[..n] != b"FVSH1 OK\n" { return Err(invalid("subscription rejected")); }
        // The subscription deadline must not accidentally expire the live stream.
        reader.socket.set_read_timeout(None)?;
        Ok(reader)
    }
    fn stream(&self, output: &mut impl Write) -> io::Result<()> {
        // One extra byte detects oversized/truncated datagrams at the upper bound.
        let mut frame = [0u8; MAX + 15];
        loop {
            let n = self.socket.recv(&mut frame)?;
            let result = payload(&frame[..n]).and_then(|pcm| output.write_all(pcm));
            frame.fill(0);
            result?;
        }
    }
}
impl Drop for Reader {
    fn drop(&mut self) { let _ = self.socket.send(b"FVSH1 UNSUB\n"); let _ = fs::remove_file(&self.local); }
}
fn run() -> io::Result<()> {
    validate_args(&env::args().skip(1).collect::<Vec<_>>())?;
    let reader = Reader::connect(Path::new(SERVER), Path::new("/tmp/open-xiaoai"))?;
    reader.stream(&mut io::stdout().lock())
}
fn main() { if let Err(error) = run() { eprintln!("fvsh-reader: {error}"); std::process::exit(1); } }

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    fn frame(bytes: &[u8]) -> Vec<u8> { [MAGIC, &(bytes.len() as u32).to_le_bytes(), bytes].concat() }
    #[test] fn frame_bounds() {
        assert_eq!(payload(&frame(&[1,2])).unwrap(), &[1,2]);
        assert!(payload(&frame(&[])).is_err()); assert!(payload(&frame(&[1])).is_err());
        assert!(payload(&frame(&vec![0; MAX+2])).is_err());
        assert!(payload(b"bad").is_err());
        let mut f=frame(&[1,2]); f.push(0); assert!(payload(&f).is_err());
    }
    #[test] fn strict_sample_contract() {
        let mut args="--quiet -t raw -D noop -f S16_LE -r 16000 -c 1 --buffer-size 1440 --period-size 360".split_whitespace().map(str::to_string).collect::<Vec<_>>();
        assert!(validate_args(&args).is_ok()); args[6]="S32_LE".into(); assert!(validate_args(&args).is_err());
    }
    #[test] fn ack_timeout_cleared_close_recovery_no_audio_files() {
        let dir=env::temp_dir().join(format!("fvsh-test-{}",std::process::id())); fs::create_dir(&dir).unwrap();
        let endpoint=dir.join("owner.sock"); let server=UnixDatagram::bind(&endpoint).unwrap();
        fs::set_permissions(&endpoint, fs::Permissions::from_mode(0o600)).unwrap();
        let worker=thread::spawn(move || {
            for _ in 0..2 {
                let mut b=[0;64]; let(n,p)=server.recv_from(&mut b).unwrap(); assert_eq!(&b[..n],b"FVSH1 SUB\n");
                let p=p.as_pathname().unwrap(); server.send_to(b"FVSH1 OK\n",p).unwrap();
                thread::sleep(Duration::from_millis(650)); server.send_to(&frame(&[1,2]),p).unwrap();
                let(n,_)=server.recv_from(&mut b).unwrap(); assert_eq!(&b[..n],b"FVSH1 UNSUB\n");
            }
        });
        for _ in 0..2 {
            let reader=Reader::connect(&endpoint,&dir).unwrap(); assert_eq!(reader.socket.read_timeout().unwrap(),None);
            let mut b=[0;64]; let n=reader.socket.recv(&mut b).unwrap(); assert_eq!(payload(&b[..n]).unwrap(),&[1,2]);
            assert!(fs::read_dir(&dir).unwrap().all(|e| e.unwrap().file_type().unwrap().is_socket()));
            let p=reader.local.clone(); drop(reader); assert!(!p.exists());
        }
        worker.join().unwrap(); fs::remove_file(endpoint).unwrap(); fs::remove_dir(dir).unwrap();
    }
}
