// SPDX-License-Identifier: MIT
//! Minimal ALSA ABI for the OH2P default PCM. Keep the firmware's own libasound.
use super::{Delay, Format, Pcm, Progress};
use std::ffi::{c_char, c_int, c_long, c_uint, c_ulong, c_void, CStr};
use std::{io, mem, ptr};

const PCM_PLAYBACK: c_int = 0;
const PCM_NONBLOCK: c_int = 1;
const ACCESS_RW_INTERLEAVED: c_int = 3;
const FORMAT_S16_LE: c_int = 2;
const STATE_PREPARED: c_int = 2;
const STATE_RUNNING: c_int = 3;
const STATE_XRUN: c_int = 4;
const STATE_DRAINING: c_int = 5;
const STATE_SUSPENDED: c_int = 7;
const EINTR: c_int = 4;
const EAGAIN: c_int = 11;
const EPIPE: c_int = 32;
const ESTRPIPE: c_int = 86;

#[link(name = "dl")]
extern "C" {
    fn dlopen(name: *const c_char, flags: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    fn dlclose(handle: *mut c_void) -> c_int;
    fn dlerror() -> *const c_char;
}

struct Library(*mut c_void);

impl Library {
    fn open() -> io::Result<Self> {
        let ptr = unsafe { dlopen(c"libasound.so.2".as_ptr(), 2) }; // RTLD_NOW
        if ptr.is_null() {
            Err(Self::error(c"libasound.so.2"))
        } else {
            Ok(Self(ptr))
        }
    }

    fn error(name: &CStr) -> io::Error {
        let error = unsafe { dlerror() };
        let detail = if error.is_null() {
            "unknown dynamic loader error".into()
        } else {
            unsafe { CStr::from_ptr(error) }.to_string_lossy()
        };
        io::Error::other(format!("{}: {detail}", name.to_string_lossy()))
    }

    fn symbol<T: Copy>(&self, name: &CStr) -> io::Result<T> {
        let ptr = unsafe { dlsym(self.0, name.as_ptr()) };
        if ptr.is_null() {
            Err(Self::error(name))
        } else {
            Ok(unsafe { mem::transmute_copy(&ptr) })
        }
    }
}

impl Drop for Library {
    fn drop(&mut self) {
        unsafe { dlclose(self.0) };
    }
}

type Handle = *mut c_void;
type Control = unsafe extern "C" fn(Handle) -> c_int;
type ParamsCall = unsafe extern "C" fn(Handle, Handle) -> c_int;
type ParamsSize = unsafe extern "C" fn(Handle, Handle, c_ulong) -> c_int;

struct Params {
    ptr: Handle,
    free: unsafe extern "C" fn(Handle),
}

impl Params {
    fn new(library: &Library, alloc: &CStr, free: &CStr) -> io::Result<Self> {
        let alloc: unsafe extern "C" fn(*mut Handle) -> c_int = library.symbol(alloc)?;
        let free = library.symbol(free)?;
        let mut ptr = ptr::null_mut();
        check(unsafe { alloc(&mut ptr) })?;
        Ok(Self { ptr, free })
    }
}

impl Drop for Params {
    fn drop(&mut self) {
        unsafe { (self.free)(self.ptr) };
    }
}

fn check(result: c_int) -> io::Result<()> {
    if result < 0 {
        Err(io::Error::from_raw_os_error(-result))
    } else {
        Ok(())
    }
}

fn configure(library: &Library, handle: Handle, requested: Format) -> io::Result<Format> {
    let any: ParamsCall = library.symbol(c"snd_pcm_hw_params_any")?;
    let access: unsafe extern "C" fn(Handle, Handle, c_int) -> c_int =
        library.symbol(c"snd_pcm_hw_params_set_access")?;
    let format: unsafe extern "C" fn(Handle, Handle, c_int) -> c_int =
        library.symbol(c"snd_pcm_hw_params_set_format")?;
    let channels: unsafe extern "C" fn(Handle, Handle, c_uint) -> c_int =
        library.symbol(c"snd_pcm_hw_params_set_channels")?;
    let rate_near: unsafe extern "C" fn(Handle, Handle, *mut c_uint, *mut c_int) -> c_int =
        library.symbol(c"snd_pcm_hw_params_set_rate_near")?;
    let period_near: unsafe extern "C" fn(Handle, Handle, *mut c_ulong, *mut c_int) -> c_int =
        library.symbol(c"snd_pcm_hw_params_set_period_size_near")?;
    let buffer_near: unsafe extern "C" fn(Handle, Handle, *mut c_ulong) -> c_int =
        library.symbol(c"snd_pcm_hw_params_set_buffer_size_near")?;
    let install_hw: ParamsCall = library.symbol(c"snd_pcm_hw_params")?;
    let get_period: unsafe extern "C" fn(Handle, *mut c_ulong, *mut c_int) -> c_int =
        library.symbol(c"snd_pcm_hw_params_get_period_size")?;
    let get_buffer: unsafe extern "C" fn(Handle, *mut c_ulong) -> c_int =
        library.symbol(c"snd_pcm_hw_params_get_buffer_size")?;
    let current: ParamsCall = library.symbol(c"snd_pcm_sw_params_current")?;
    let avail_min: ParamsSize = library.symbol(c"snd_pcm_sw_params_set_avail_min")?;
    let start_threshold: ParamsSize = library.symbol(c"snd_pcm_sw_params_set_start_threshold")?;
    let stop_threshold: ParamsSize = library.symbol(c"snd_pcm_sw_params_set_stop_threshold")?;
    let install_sw: ParamsCall = library.symbol(c"snd_pcm_sw_params")?;

    let hw = Params::new(
        library,
        c"snd_pcm_hw_params_malloc",
        c"snd_pcm_hw_params_free",
    )?;
    let mut rate = requested.rate;
    let mut period = c_ulong::from(requested.period);
    let mut buffer = c_ulong::from(requested.buffer);
    unsafe {
        check(any(handle, hw.ptr))?;
        check(access(handle, hw.ptr, ACCESS_RW_INTERLEAVED))?;
        check(format(handle, hw.ptr, FORMAT_S16_LE))?;
        check(channels(handle, hw.ptr, 1))?;
        check(rate_near(handle, hw.ptr, &mut rate, ptr::null_mut()))?;
        check(period_near(handle, hw.ptr, &mut period, ptr::null_mut()))?;
        check(buffer_near(handle, hw.ptr, &mut buffer))?;
        check(install_hw(handle, hw.ptr))?;
        check(get_period(hw.ptr, &mut period, ptr::null_mut()))?;
        check(get_buffer(hw.ptr, &mut buffer))?;
    }
    if rate != requested.rate {
        return Err(io::Error::other(format!(
            "ALSA selected {rate} Hz for {} Hz PCM",
            requested.rate
        )));
    }
    if period == 0 || period >= buffer || buffer > c_ulong::from(rate) {
        return Err(io::Error::other("unexpected ALSA period or buffer size"));
    }

    let sw = Params::new(
        library,
        c"snd_pcm_sw_params_malloc",
        c"snd_pcm_sw_params_free",
    )?;
    // Match native aplay's default start/stop thresholds and period wakeup.
    unsafe {
        check(current(handle, sw.ptr))?;
        check(avail_min(handle, sw.ptr, period))?;
        check(start_threshold(handle, sw.ptr, buffer))?;
        check(stop_threshold(handle, sw.ptr, buffer))?;
        check(install_sw(handle, sw.ptr))?;
    }
    Ok(Format {
        rate,
        buffer: buffer as u32,
        period: period as u32,
    })
}

struct Calls {
    close: Control,
    write: unsafe extern "C" fn(Handle, *const c_void, c_ulong) -> c_long,
    delay: unsafe extern "C" fn(Handle, *mut c_long) -> c_int,
    state: Control,
    drain: Control,
    start: Control,
    prepare: Control,
    resume: Control,
}

pub(super) struct Alsa {
    handle: Handle,
    calls: Calls,
    _library: Library,
}

impl Alsa {
    pub(super) fn open(format: Format) -> io::Result<(Self, Format)> {
        let library = Library::open()?;
        let open: unsafe extern "C" fn(*mut Handle, *const c_char, c_int, c_int) -> c_int =
            library.symbol(c"snd_pcm_open")?;
        let calls = Calls {
            close: library.symbol(c"snd_pcm_close")?,
            write: library.symbol(c"snd_pcm_writei")?,
            delay: library.symbol(c"snd_pcm_delay")?,
            state: library.symbol(c"snd_pcm_state")?,
            drain: library.symbol(c"snd_pcm_drain")?,
            start: library.symbol(c"snd_pcm_start")?,
            prepare: library.symbol(c"snd_pcm_prepare")?,
            resume: library.symbol(c"snd_pcm_resume")?,
        };
        let mut handle = ptr::null_mut();
        check(unsafe { open(&mut handle, c"default".as_ptr(), PCM_PLAYBACK, PCM_NONBLOCK) })?;
        let pcm = Self {
            handle,
            calls,
            _library: library,
        };
        let actual = configure(&pcm._library, pcm.handle, format)?;
        Ok((pcm, actual))
    }

    fn control(&self, function: Control) -> c_int {
        unsafe { function(self.handle) }
    }

    fn recover<T>(&mut self, error: c_int) -> io::Result<Progress<T>> {
        match -error {
            EINTR | EAGAIN => Ok(Progress::Retry),
            ESTRPIPE => {
                let resumed = self.control(self.calls.resume);
                if resumed >= 0 || resumed == -EAGAIN || resumed == -EINTR {
                    return Ok(Progress::Retry);
                }
                check(self.control(self.calls.prepare))?;
                Ok(Progress::Reset)
            }
            EPIPE => {
                check(self.control(self.calls.prepare))?;
                Ok(Progress::Reset)
            }
            _ => Err(io::Error::from_raw_os_error(-error)),
        }
    }
}

impl Pcm for Alsa {
    fn write(&mut self, samples: &[u8]) -> io::Result<Progress<usize>> {
        let frames = (samples.len() / 2) as c_ulong;
        let result = unsafe { (self.calls.write)(self.handle, samples.as_ptr().cast(), frames) };
        if result == 0 {
            Ok(Progress::Retry)
        } else if result < 0 {
            self.recover(result as c_int)
        } else {
            Ok(Progress::Ready(result as usize))
        }
    }

    fn delay(&mut self) -> Delay {
        let mut frames = 0;
        let result = unsafe { (self.calls.delay)(self.handle, &mut frames) };
        // Old rate plugins can return a delay even while their slave is in XRUN.
        // Read state after delay, which may itself update the hardware pointer.
        match self.control(self.calls.state) {
            STATE_XRUN => Delay::Reset,
            STATE_SUSPENDED => Delay::Unknown,
            STATE_PREPARED if result >= 0 => Delay::Prepared,
            STATE_RUNNING | STATE_DRAINING if result >= 0 && frames >= 0 => {
                Delay::Queued(frames as usize)
            }
            _ => Delay::Unknown,
        }
    }

    fn start(&mut self) -> io::Result<Progress<()>> {
        let result = self.control(self.calls.start);
        if result < 0 {
            self.recover(result)
        } else {
            Ok(Progress::Ready(()))
        }
    }

    fn drain(&mut self) -> io::Result<Progress<()>> {
        let result = self.control(self.calls.drain);
        if result < 0 {
            self.recover(result)
        } else {
            Ok(Progress::Ready(()))
        }
    }
}

impl Drop for Alsa {
    fn drop(&mut self) {
        self.control(self.calls.close);
    }
}
