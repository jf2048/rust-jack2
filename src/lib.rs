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
    pub fn from_micros(usecs: u64) -> Self {
        Self(usecs)
    }
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
    #[inline]
    pub fn new(uuid: std::num::NonZeroU64) -> Self {
        Self(uuid)
    }
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

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    InvalidCString(#[from] std::ffi::NulError),
    #[error(transparent)]
    LibraryError(#[from] &'static libloading::Error),
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
