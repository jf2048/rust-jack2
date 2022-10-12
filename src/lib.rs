//! # jack2
//!
//! A safe Rust binding for [JACK Audio Connection Kit](https://jackaudio.org) with some advanced
//! features.
//!
//! JACK is a pro-audio sound server for sending real-time audio between applications. The JACK API
//! is provided in a C library that allows clients to connect to the server and start exchanging
//! audio data. This crate provides safe Rust bindings for the C API. See the JACK
//! [FAQ](https://jackaudio.org/faq/) and
//! [Wiki](https://github.com/jackaudio/jackaudio.github.com/wiki) for more information the overall
//! project. [Pipewire](https://pipewire.org/) is also supported, as it implements a compatible
//! subset of the JACK API.
//!
//! JACK clients work by reading and filling audio buffers in real-time threads, that are scheduled
//! by the server to produce audio at fixed intervals on a strict deadline. To avoid "skips" in the
//! audio, real-time threads must make sure to only call functions that are thread-safe and
//! real-time-safe. But the C API for JACK can be cumbersome to use, as the C language does not
//! have a way to express which functions have those characteristics. Without careful reading of the
//! documentation, it can lead to bugs and glitchy audio in clients.
//!
//! On the other hand, Rust has powerful tools built into the language to ensure thread-safety.
//! This crate has been designed to make it difficult to accidentally call non-thread-safe and
//! non-real-time-safe methods in real-time audio processing threads. Unfortunately, Rust does not
//! have the ability to statically check if all methods are fully real-time-safe. While this crate
//! does take steps to prevent users from calling non-real-time-safe JACK functions in the wrong
//! context, care must still be taken not to accidentally call Rust functions from other libraries
//! (including [`std`]) that might take too much time in real-time threads. Overall, using this crate
//! should still provide some safety and usability enhancements over the C API, without adding any
//! extra performance overhead.
//!
//! The goal of this binding is to have 100% safe API coverage for the subset of all API features
//! supported by both [`jackd2`](https://github.com/jackaudio/jack2) and by
//! [`pipewire-jack`](https://gitlab.freedesktop.org/pipewire/pipewire/-/wikis/Config-JACK). These
//! bindings will not support obscure or deprecated libjack features, such as internal clients,
//! session management, netjack, client threads, or jackctl. Features present only in
//! [`jackd1`](https://github.com/jackaudio/jack1) will not be supported. The JACK ringbuffer
//! is also not supported; use one of the many native Rust ring buffer or bounded channel
//! implementations instead.
//!
//! ## Usage
//!
//! Users of this API will typically start by using [`ClientBuilder`] to configure and open a
//! connection to the JACK server. Then, the [`Client`] can be used to create "ports" to
//! send/receive audio data. Clients usually need to supply a real-time-safe "process
//! callback" to perform the actual reading and writing of audio data. This can be done either by
//! implementing [`ProcessHandler::process`], or by using [`ClosureProcessHandler`]. The handler
//! is then passed to [`InactiveClient::activate`] when configuring the client.
//!
//! Clients can add and remove ports using [`Client::register_ports`] and
//! [`Client::unregister_ports`]. The mechanism used by these methods ensures that ports that are
//! added/removed in a group are seen as an atomic update in the process thread. For complex
//! clients that need to frequently add or remove ports (such as DAWs), this ensures that no audio
//! skips, glitches or mid-frame cut-outs happen.
//!
//! Data required for processing the port can also be provided as a ["port
//! data"](ProcessHandler::PortData) type at the time of port registration. This ensures that the
//! process thread always has a reference to the data it needs to run the graph, and also ensures
//! that this data is deallocated correctly when unregistering the port. That is, if a port is
//! unregistered while the process cycle is running, the data is automatically sent back to the
//! main thread and deallocated after the process cycle finishes.
//!
//! Clients can also make use of some advanced server features, including:
//!
//! - Controlling the global audio transport with [`Transport`].
//! - Using [`NotificationHandler`] to receive events when the global server state changes.
//! - Setting and retrieving [metadata] properties on clients and ports.
//!
//! See the documentation of [`Client`] for a full list of available API functionality.
//!
//! ## Prior Art
//!
//! The [`jack`](https://docs.rs/jack/) crate was written several years before the `jack2` crate,
//! by a different author. The `jack2` crate is not based on it, but it does take inspiration from
//! some concepts when it makes sense, and some type names are intentionally kept similar for ease
//! of migration. The main difference to the `jack` crate is in how ports are handled, using the
//! atomic updating strategy described in the above paragraphs. This crate could be compared to a
//! complete rewrite of `jack`, with a cleaned-up API.

pub mod sys;

mod callbacks;
pub use callbacks::*;
mod client;
pub use client::*;
pub mod metadata;
mod port;
pub use port::*;
mod process;
pub use process::*;
mod properties;
pub use properties::*;
mod transport;
pub use transport::*;

#[cfg(feature = "glib")]
pub mod glib;
#[cfg(feature = "tokio")]
pub mod tokio;

#[doc(alias = "jack_nframes_t")]
#[repr(transparent)]
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct Frames(sys::jack_nframes_t);

impl From<Frames> for sys::jack_nframes_t {
    #[inline]
    fn from(value: Frames) -> Self {
        value.0
    }
}

impl From<sys::jack_nframes_t> for Frames {
    #[inline]
    fn from(value: sys::jack_nframes_t) -> Self {
        Self(value)
    }
}

#[doc(alias = "jack_time_t")]
#[repr(transparent)]
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct Time(sys::jack_time_t);

impl Time {
    #[doc(alias = "jack_get_time")]
    pub fn now() -> Self {
        Self(unsafe { sys::library().jack_get_time() })
    }
    /// Creates a time value from a value in microseconds.
    pub fn from_micros(usecs: u64) -> Self {
        Self(usecs)
    }
    /// Returns the value in microseconds as a 64-bit unsigned integer.
    pub fn as_micros(&self) -> u64 {
        self.0
    }
}

impl From<Time> for std::time::Duration {
    #[inline]
    fn from(value: Time) -> Self {
        std::time::Duration::from_micros(value.0)
    }
}

impl TryFrom<std::time::Duration> for Time {
    type Error = std::num::TryFromIntError;
    #[inline]
    fn try_from(value: std::time::Duration) -> std::result::Result<Self, Self::Error> {
        Ok(Self(u64::try_from(value.as_micros())?))
    }
}

/// Type representing a 64-bit unique identifier.
#[doc(alias = "jack_uuid_t")]
#[repr(transparent)]
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct Uuid(std::num::NonZeroU64);

impl Uuid {
    unsafe fn from_raw(ptr: *const std::os::raw::c_char) -> Option<Self> {
        if ptr.is_null() {
            return None;
        }
        let uuid = str::parse(std::str::from_utf8_unchecked(
            std::ffi::CStr::from_ptr(ptr).to_bytes(),
        ))
        .unwrap();
        sys::library().jack_free(ptr as *mut _);
        Some(uuid)
    }
    /// Create a new unique identifier from a non-zero 64-bit unsigned integer.
    #[inline]
    pub fn new(uuid: std::num::NonZeroU64) -> Self {
        Self(uuid)
    }
    /// Return the numeric value of the identifier.
    #[inline]
    pub fn value(&self) -> std::num::NonZeroU64 {
        self.0
    }
}

impl std::fmt::Display for Uuid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut buf = std::mem::MaybeUninit::<
            [std::os::raw::c_char; sys::JACK_UUID_STRING_SIZE as usize],
        >::uninit();
        let s = unsafe {
            let ptr = buf.as_mut_ptr() as *mut std::os::raw::c_char;
            sys::weak_library()
                .map_err(|_| std::fmt::Error)?
                .jack_uuid_unparse(self.0.get(), ptr);
            let cstr = std::ffi::CStr::from_ptr(ptr as *const _);
            std::str::from_utf8_unchecked(cstr.to_bytes())
        };
        s.fmt(f)
    }
}

impl std::str::FromStr for Uuid {
    type Err = Error;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        let mut uuid = 0;
        let s = std::ffi::CString::new(&s[0..s.len().min(36)])?;
        let ret = unsafe { sys::weak_library()?.jack_uuid_parse(s.as_ptr(), &mut uuid) };
        if ret < 0 {
            return Err(Error::invalid_option());
        }
        Ok(Self(
            std::num::NonZeroU64::new(uuid).ok_or_else(Error::invalid_option)?,
        ))
    }
}

struct JackStr(std::ptr::NonNull<std::os::raw::c_char>);

impl JackStr {
    unsafe fn from_raw_unchecked(ptr: *mut std::os::raw::c_char) -> Self {
        Self(std::ptr::NonNull::new_unchecked(ptr))
    }
    unsafe fn as_cstring(&self) -> std::ffi::CString {
        std::ffi::CStr::from_ptr(self.0.as_ptr()).into()
    }
}

impl Drop for JackStr {
    #[inline]
    fn drop(&mut self) {
        unsafe {
            sys::library().jack_free(self.0.as_ptr() as *mut _);
        }
    }
}

/// Error enum for all public errors in this crate.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A C string was passed without a valid nul-terminator.
    #[error(transparent)]
    InvalidCString(#[from] std::ffi::NulError),
    /// The JACK library failed to load.
    #[error(transparent)]
    LibraryError(#[from] &'static libloading::Error),
    /// A generic failure from a JACK API function.
    #[error(transparent)]
    Failure(#[from] Failure),
}

impl Error {
    #[inline]
    fn unknown() -> Self {
        Failure(sys::JackStatus_JackFailure).into()
    }
    #[inline]
    fn invalid_option() -> Self {
        Failure(sys::JackStatus_JackFailure | sys::JackStatus_JackInvalidOption).into()
    }
    #[inline]
    fn check_ret(ret: std::os::raw::c_int) -> Result<()> {
        if ret < 0 {
            return Err(Self::unknown());
        }
        Ok(())
    }
}

/// Result type for [`crate::Error`].
pub type Result<T> = std::result::Result<T, Error>;

#[doc(alias = "jack_status_t")]
#[doc(alias = "JackStatus")]
#[repr(transparent)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Failure(sys::jack_status_t);

impl std::error::Error for Failure {}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if (self.0 & sys::JackStatus_JackInvalidOption) != 0 {
            "The operation contained an invalid or unsupported option"
        } else if (self.0 & sys::JackStatus_JackNameNotUnique) != 0 {
            "The desired client name was not unique"
        } else if (self.0 & sys::JackStatus_JackServerFailed) != 0 {
            "Unable to connect to the JACK server"
        } else if (self.0 & sys::JackStatus_JackServerError) != 0 {
            "Communication error with the JACK server"
        } else if (self.0 & sys::JackStatus_JackNoSuchClient) != 0 {
            "Requested client does not exist"
        } else if (self.0 & sys::JackStatus_JackLoadFailure) != 0 {
            "Unable to load internal client"
        } else if (self.0 & sys::JackStatus_JackInitFailure) != 0 {
            "Unable to initialize client"
        } else if (self.0 & sys::JackStatus_JackShmFailure) != 0 {
            "Unable to access shared memory"
        } else if (self.0 & sys::JackStatus_JackVersionError) != 0 {
            "Client's protocol version does not match"
        } else if (self.0 & sys::JackStatus_JackBackendError) != 0 {
            "Backend error"
        } else if (self.0 & sys::JackStatus_JackClientZombie) != 0 {
            "Client zombified failure"
        } else {
            "Unknown error"
        }
        .fmt(f)
    }
}

impl From<Failure> for sys::jack_status_t {
    #[inline]
    fn from(status: Failure) -> Self {
        status.0
    }
}

#[doc(alias = "jack_get_version")]
pub fn version() -> Result<(i32, i32, i32, i32)> {
    let mut major = 0;
    let mut minor = 0;
    let mut micro = 0;
    let mut proto = 0;
    unsafe {
        sys::weak_library()?.jack_get_version(&mut major, &mut minor, &mut micro, &mut proto);
    }
    Ok((major, minor, micro, proto))
}

#[doc(alias = "jack_get_version_string")]
pub fn version_string() -> Result<&'static std::ffi::CStr> {
    Ok(unsafe { std::ffi::CStr::from_ptr(sys::weak_library()?.jack_get_version_string()) })
}

#[doc(alias = "jack_client_name_size")]
pub fn client_name_size() -> Result<usize> {
    Ok(unsafe { sys::weak_library()?.jack_client_name_size() as usize })
}

#[doc(alias = "jack_port_name_size")]
pub fn port_name_size() -> Result<usize> {
    Ok(unsafe { sys::weak_library()?.jack_port_name_size() as usize })
}

#[doc(alias = "jack_port_type_size")]
pub fn port_type_size() -> Result<usize> {
    Ok(unsafe { sys::weak_library()?.jack_port_type_size() as usize })
}
