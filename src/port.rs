use arrayvec::ArrayVec;
use std::{
    ffi::{CStr, CString},
    marker::PhantomData,
    mem::MaybeUninit,
    num::{NonZeroU32, NonZeroU64},
    os::raw::c_char,
    ptr::NonNull,
};

use crate::{
    sys::{self, library},
    ClientHandle, Error, Frames, LatencyMode, Uuid,
};

#[repr(transparent)]
#[derive(Debug)]
pub struct Port<'c> {
    port: NonNull<sys::jack_port_t>,
    client: PhantomData<&'c ClientHandle>,
}

impl<'c> Port<'c> {
    pub(crate) fn new(_client: &'c ClientHandle, port: *mut sys::jack_port_t) -> Option<Self> {
        Some(Self {
            port: NonNull::new(port)?,
            client: PhantomData,
        })
    }
    #[inline]
    pub fn as_ptr(&self) -> NonNull<sys::jack_port_t> {
        self.port
    }
    pub fn uuid(&self) -> Uuid {
        let uuid =
            unsafe { NonZeroU64::new_unchecked(library().jack_port_uuid(self.port.as_ptr())) };
        Uuid(uuid)
    }
    pub fn name(&self) -> CString {
        let name = unsafe { library().jack_port_name(self.port.as_ptr()) };
        debug_assert!(!name.is_null());
        unsafe { CStr::from_ptr(name) }.into()
    }
    pub fn short_name(&self) -> CString {
        let name = unsafe { library().jack_port_short_name(self.port.as_ptr()) };
        debug_assert!(!name.is_null());
        unsafe { CStr::from_ptr(name) }.into()
    }
    pub fn flags(&self) -> PortFlags {
        unsafe {
            PortFlags::from_bits_unchecked(
                library().jack_port_flags(self.port.as_ptr()) as sys::JackPortFlags
            )
        }
    }
    pub fn type_id(&self) -> Option<PortType> {
        let id = unsafe { library().jack_port_type_id(self.port.as_ptr()) };
        match id {
            0 => Some(PortType::Audio),
            1 => Some(PortType::Midi),
            _ => None,
        }
    }
    pub fn type_name(&self) -> Option<&CStr> {
        let ty = unsafe { library().jack_port_type(self.port.as_ptr()) };
        if ty.is_null() {
            None
        } else {
            Some(unsafe { CStr::from_ptr(ty) })
        }
    }
    pub fn connected(&self) -> bool {
        (unsafe { library().jack_port_connected(self.port.as_ptr()) }) == 1
    }
    pub fn connected_to(&self, port_name: impl AsRef<CStr>) -> bool {
        (unsafe {
            library().jack_port_connected_to(self.port.as_ptr(), port_name.as_ref().as_ptr())
        }) == 1
    }
    pub fn connections(&self) -> Vec<CString> {
        PortList(unsafe { library().jack_port_get_connections(self.port.as_ptr()) }).to_vec()
    }
    pub fn set_alias(&self, alias: impl AsRef<CStr>) -> crate::Result<()> {
        Error::check_ret(unsafe {
            library().jack_port_set_alias(self.port.as_ptr(), alias.as_ref().as_ptr())
        })
    }
    pub fn unset_alias(&self, alias: impl AsRef<CStr>) -> crate::Result<()> {
        Error::check_ret(unsafe {
            library().jack_port_unset_alias(self.port.as_ptr(), alias.as_ref().as_ptr())
        })
    }
    pub fn aliases(&self) -> crate::Result<ArrayVec<CString, 2>> {
        extern "C" {
            fn strlen(s: *const c_char) -> usize;
        }
        let name_size = unsafe { library().jack_port_name_size() } as usize;
        let mut a1 = Vec::<u8>::with_capacity(name_size);
        let mut a2 = Vec::<u8>::with_capacity(name_size);
        let aliases = unsafe {
            [
                a1.spare_capacity_mut().get_unchecked_mut(0).as_mut_ptr() as *mut c_char,
                a2.spare_capacity_mut().get_unchecked_mut(0).as_mut_ptr() as *mut c_char,
            ]
        };
        let count =
            unsafe { library().jack_port_get_aliases(self.port.as_ptr(), aliases.as_ptr()) };
        Error::check_ret(count)?;
        let mut av = ArrayVec::new();
        if count > 0 {
            unsafe {
                a1.set_len(strlen(a1.as_ptr() as *const c_char) + 1);
                av.push(CString::from_vec_with_nul_unchecked(a1));
            }
        }
        if count > 1 {
            unsafe {
                a2.set_len(strlen(a2.as_ptr() as *const c_char) + 1);
                av.push(CString::from_vec_with_nul_unchecked(a2));
            }
        }
        Ok(av)
    }
    pub fn request_monitor(&self, onoff: bool) -> crate::Result<()> {
        Error::check_ret(unsafe {
            library().jack_port_request_monitor(self.port.as_ptr(), onoff as _)
        })
    }
    pub fn ensure_monitor(&self, onoff: bool) -> crate::Result<()> {
        Error::check_ret(unsafe {
            library().jack_port_request_monitor(self.port.as_ptr(), onoff as _)
        })
    }
    pub fn monitoring_input(&self) -> bool {
        (unsafe { library().jack_port_monitoring_input(self.port.as_ptr()) }) == 1
    }
    pub fn latency_range(&self, mode: LatencyMode) -> LatencyRange {
        let mut range = MaybeUninit::<sys::jack_latency_range_t>::uninit();
        let range = unsafe {
            library().jack_port_get_latency_range(
                self.port.as_ptr(),
                mode.into_jack(),
                range.as_mut_ptr(),
            );
            range.assume_init()
        };
        LatencyRange {
            min: Frames(range.min),
            max: Frames(range.max),
        }
    }
    pub fn set_latency_range(&self, mode: LatencyMode, range: LatencyRange) {
        let mut range = sys::jack_latency_range_t {
            min: range.min.into(),
            max: range.max.into(),
        };
        unsafe {
            library().jack_port_set_latency_range(self.port.as_ptr(), mode.into_jack(), &mut range);
        }
    }
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
#[repr(transparent)]
pub struct PortId(pub(crate) NonZeroU32);

bitflags::bitflags! {
    pub struct PortFlags: sys::JackPortFlags {
        const IS_INPUT    = sys::JackPortFlags_JackPortIsInput;
        const IS_OUTPUT   = sys::JackPortFlags_JackPortIsOutput;
        const PHYSICAL    = sys::JackPortFlags_JackPortIsPhysical;
        const CAN_MONITOR = sys::JackPortFlags_JackPortCanMonitor;
        const IS_TERMINAL = sys::JackPortFlags_JackPortIsTerminal;
    }
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub enum PortType {
    Audio,
    Midi,
}

impl PortType {
    pub(crate) fn as_cstr(&self) -> &'static CStr {
        let ty = match self {
            Self::Audio => sys::JACK_DEFAULT_AUDIO_TYPE.as_slice(),
            Self::Midi => sys::JACK_DEFAULT_MIDI_TYPE.as_slice(),
        };
        unsafe { CStr::from_bytes_with_nul_unchecked(ty) }
    }
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub enum PortMode {
    Input,
    Output,
}

impl PortMode {
    pub(crate) fn flags(&self) -> sys::JackPortFlags {
        match self {
            Self::Input => sys::JackPortFlags_JackPortIsInput,
            Self::Output => sys::JackPortFlags_JackPortIsOutput,
        }
    }
}

#[derive(Debug)]
pub struct LatencyRange {
    pub min: Frames,
    pub max: Frames,
}

pub(crate) struct PortList(pub *mut *const std::os::raw::c_char);

impl PortList {
    pub fn to_vec(&self) -> Vec<CString> {
        let mut names = Vec::new();
        let mut iter = self.0;
        while !iter.is_null() {
            unsafe {
                let ptr = iter.read();
                names.push(CStr::from_ptr(ptr).into());
                iter = iter.add(1);
            }
        }
        names
    }
}

impl Drop for PortList {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { library().jack_free(self.0 as *mut _) }
        }
    }
}
