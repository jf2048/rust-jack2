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

/// A reference to a JACK port owned by any client on the current server.
///
/// References are short lived and should not be stored. Only port IDs or port names should be
/// stored by the client Then a reference to a port can be obtained with
/// [`Client::port_by_id`](crate::Client::port_by_id) or
/// [`Client::port_by_name`](crate::Client::port_by_name) whe.
///
/// Other clients can remove ports or close down at any time, so care should be taken to listen for
/// [`PortUnregister`](crate::Notification::PortUnregister) notifications and avoid using [`Port`]
/// objects after the port has been unregistered.
#[doc(alias = "jack_port_t")]
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
    /// Returns the 64-bit unique ID of the port.
    #[doc(alias = "jack_port_uuid")]
    pub fn uuid(&self) -> Uuid {
        let uuid =
            unsafe { NonZeroU64::new_unchecked(library().jack_port_uuid(self.port.as_ptr())) };
        Uuid(uuid)
    }
    /// Returns the full name of the port (including the `client_name:` prefix).
    #[doc(alias = "jack_port_name")]
    pub fn name(&self) -> CString {
        let name = unsafe { library().jack_port_name(self.port.as_ptr()) };
        debug_assert!(!name.is_null());
        unsafe { CStr::from_ptr(name) }.into()
    }
    /// Returns the short name of the port (not including the `client_name:` prefix).
    #[doc(alias = "jack_port_short_name")]
    pub fn short_name(&self) -> CString {
        let name = unsafe { library().jack_port_short_name(self.port.as_ptr()) };
        debug_assert!(!name.is_null());
        unsafe { CStr::from_ptr(name) }.into()
    }
    /// Returns the flags used when registering the port.
    #[doc(alias = "jack_port_flags")]
    pub fn flags(&self) -> PortFlags {
        unsafe { PortFlags(library().jack_port_flags(self.port.as_ptr()) as sys::JackPortFlags) }
    }
    /// Returns the data type of the port.
    ///
    /// Returns `None` if the port is not an audio or MIDI port. In that case, use
    /// [`Self::type_name`] to identify the port type.
    #[doc(alias = "jack_port_type_id")]
    pub fn type_id(&self) -> Option<PortType> {
        let id = unsafe { library().jack_port_type_id(self.port.as_ptr()) };
        match id {
            0 => Some(PortType::Audio),
            1 => Some(PortType::Midi),
            _ => None,
        }
    }
    /// Returns a human-readable string identifying the port type.
    #[doc(alias = "jack_port_type")]
    pub fn type_name(&self) -> Option<&CStr> {
        let ty = unsafe { library().jack_port_type(self.port.as_ptr()) };
        if ty.is_null() {
            None
        } else {
            Some(unsafe { CStr::from_ptr(ty) })
        }
    }
    /// Returns the number of connections to or from the port.
    ///
    /// The calling client must own the port.
    #[doc(alias = "jack_port_connected")]
    pub fn connected(&self) -> u32 {
        unsafe { library().jack_port_connected(self.port.as_ptr()) }
            .try_into()
            .unwrap()
    }
    /// Returns `true` if the locally-owned port is directly connected to the port with the full name `port_name`.
    ///
    /// The calling client must own the port in `self`.
    #[doc(alias = "jack_port_connected_to")]
    pub fn connected_to(&self, port_name: impl AsRef<CStr>) -> bool {
        (unsafe {
            library().jack_port_connected_to(self.port.as_ptr(), port_name.as_ref().as_ptr())
        }) == 1
    }
    /// Returns a list of full port names to which the port is connected.
    #[doc(alias = "jack_port_get_connections")]
    pub fn connections(&self) -> Vec<CString> {
        PortList(unsafe { library().jack_port_get_connections(self.port.as_ptr()) }).to_vec()
    }
    /// Set an alias for the port.
    ///
    /// May be called at any time. If the alias is longer than [`crate::port_name_size`], it will
    /// be truncated.
    ///
    /// After a successful call, and until JACK exits or [`Self::unset_alias`] is called, `alias`
    /// may be used as a alternate name for the port.
    ///
    /// Ports can have up to two aliases. If both are already set, this method will return `Err`.
    #[doc(alias = "jack_port_set_alias")]
    pub fn set_alias(&self, alias: impl AsRef<CStr>) -> crate::Result<()> {
        Error::check_ret(unsafe {
            library().jack_port_set_alias(self.port.as_ptr(), alias.as_ref().as_ptr())
        })
    }
    /// Remove alias an alias for the port.
    ///
    /// May be called at any time. After a successful call, `alias` can no longer be used as a
    /// alternate name for the port.
    #[doc(alias = "jack_port_unset_alias")]
    pub fn unset_alias(&self, alias: impl AsRef<CStr>) -> crate::Result<()> {
        Error::check_ret(unsafe {
            library().jack_port_unset_alias(self.port.as_ptr(), alias.as_ref().as_ptr())
        })
    }
    /// Get any aliases known for the port.
    ///
    /// Returns a vector of at most two aliases.
    #[doc(alias = "jack_port_get_aliases")]
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
    /// Turn port input monitoring on or off.
    ///
    /// Does nothing if [`PortFlags::can_monitor`] is not set for this port.
    #[doc(alias = "jack_port_request_monitor")]
    pub fn request_monitor(&self, onoff: bool) -> crate::Result<()> {
        Error::check_ret(unsafe {
            library().jack_port_request_monitor(self.port.as_ptr(), onoff as _)
        })
    }
    /// Turns on input monitoring if it was off, and turns it off if only one request has been made
    /// to turn it on.
    ///
    /// Does nothing if [`PortFlags::can_monitor`] is not set for this port.
    #[doc(alias = "jack_port_ensure_monitor")]
    pub fn ensure_monitor(&self, onoff: bool) -> crate::Result<()> {
        Error::check_ret(unsafe {
            library().jack_port_request_monitor(self.port.as_ptr(), onoff as _)
        })
    }
    /// Returns `true` if input monitoring has been requested for this port.
    #[doc(alias = "jack_port_monitoring_input")]
    pub fn monitoring_input(&self) -> bool {
        (unsafe { library().jack_port_monitoring_input(self.port.as_ptr()) }) == 1
    }
    /// Returns the latency range defined by `mode` for the port, in frames.
    ///
    /// See [`LatencyMode`] for the definition of each latency value.
    ///
    /// This method is best in a
    /// [`NotificationHandler::latency`](crate::NotificationHandler::latency) implementation.
    /// Before a port is connected, this returns the default latency: zero. Therefore it only makes
    /// sense to call `latency_range` when the port is connected, and that gets signalled by the
    /// latency notification. See [`Notification::Latency`](crate::Notification::Latency) for
    /// details.
    #[doc(alias = "jack_port_get_latency_range")]
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
    /// Set the minimum and maximum latencies defined by ` mode ` for the port, in frames.
    ///
    /// See [`LatencyMode`] for the definition of each latency value.
    ///
    /// This method should *only* be used in a handler for
    /// [`Notification::Latency`](crate::Notification::Latency) or a
    /// [`NotificationHandler::latency`](crate::NotificationHandler::latency) implementation. The
    /// client should determine the current value of the latency using [`Self::latency_range`]
    /// (called using the same mode as `mode`) and then add some number of frames to that reflects
    /// latency added by the client.
    ///
    /// How much latency a client adds will vary dramatically. For most clients, the answer is zero
    /// and there is no reason for them to register a latency callback and thus they should never
    /// call this method.
    ///
    /// More complex clients that take an input signal, transform it in some way and output the
    /// result but not during the same [`ProcessHandler::process`](crate::ProcessHandler::process)
    /// call will generally know a single constant value to add to the value returned by
    /// [`Self::latency_range`].
    ///
    /// Such clients would register a latency callback (see
    /// [`ActivateFlags::LATENCY`](crate::ActivateFlags::LATENCY)) and must know what input ports
    /// feed which output ports as part of their internal state. Their latency callback will update
    /// the ports' latency values appropriately.
    ///
    /// A pseudo-code example will help. The `mode` argument to the latency callback will
    /// determine whether playback or capture latency is being set. The callback will use
    /// `set_latency_range` as follows:
    ///
    /// In this relatively simple pseudo-code example, it is assumed that each input port or output
    /// is connected to only 1 output or input port respectively.
    ///
    /// ```ignore
    /// if mode == LatencyMode::Playback {
    ///   for input_port in &all_owned_ports {
    ///     let mut range = port_feeding_input_port.latency_range(LatencyMode::Playback);
    ///     range.min += min_delay_added_as_signal_flows_from_port_feeding_to_input_port;
    ///     range.max += max_delay_added_as_signal_flows_from_port_feeding_to_input_port;
    ///     input_port.set_latency_range(LatencyMode::Playback, range);
    ///   }
    /// } else if mode == LatencyMode::Capture {
    ///   for output_port in &all_owned_ports {
    ///     let mut range = port_fed_by_output_port.latency_range(LatencyMode::Capture);
    ///     range.min += min_delay_added_as_signal_flows_from_output_port_to_fed_by_port;
    ///     range.max += max_delay_added_as_signal_flows_from_output_port_to_fed_by_port;
    ///     output_port.set_latency_range(LatencyMode::Capture, range);
    ///   }
    /// }
    /// ```
    ///
    /// If a port is connected to more than 1 other port, then the [`range.min`] and [`range.max`]
    /// values passed to `set_latency_range` should reflect the minimum and maximum values across
    /// all connected ports.
    ///
    /// See [`Notification::Latency`](crate::Notification::Latency) for more information.
    #[doc(alias = "jack_port_set_latency_range")]
    pub fn set_latency_range(&self, mode: LatencyMode, range: LatencyRange) {
        let mut range = sys::jack_latency_range_t {
            min: range.min.into(),
            max: range.max.into(),
        };
        unsafe {
            library().jack_port_set_latency_range(self.port.as_ptr(), mode.into_jack(), &mut range);
        }
    }
    /// Returns the C pointer corresponding to this port.
    #[inline]
    pub fn as_ptr(&self) -> NonNull<sys::jack_port_t> {
        self.port
    }
}

/// A unique ID to track JACK ports.
///
/// Port [notifications](crate::Notification) are the only place where IDs are used to track ports.
/// Clients can store this ID as a means to track external ports.
#[doc(alias = "jack_port_id_t")]
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
#[repr(transparent)]
pub struct PortId(pub(crate) NonZeroU32);

/// A set of flags describing JACK ports.
///
/// The `is_input` and `is_output` flags are mutually exclusive, and ports will return `true` for
/// only one of them.
#[doc(alias = "JackPortFlags")]
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
#[repr(transparent)]
pub struct PortFlags(pub(crate) sys::JackPortFlags);

impl PortFlags {
    /// Returns `true` if the port can receive data.
    ///
    /// Mutually exclusive with [`Self::is_output`].
    #[doc(alias = "JackPortIsInput")]
    #[inline]
    pub fn is_input(&self) -> bool {
        (self.0 & sys::JackPortFlags_JackPortIsInput) != 0
    }
    /// Returns `true` if data can be read from the port.
    ///
    /// Mutually exclusive with [`Self::is_input`].
    #[doc(alias = "JackPortIsOutput")]
    #[inline]
    pub fn is_output(&self) -> bool {
        (self.0 & sys::JackPortFlags_JackPortIsOutput) != 0
    }
    /// Returns `true` if the port corresponds to some kind of physical I/O connector.
    #[inline]
    #[doc(alias = "JackPortIsPhysical")]
    pub fn is_physical(&self) -> bool {
        (self.0 & sys::JackPortFlags_JackPortIsPhysical) != 0
    }
    /// Returns `true` if [`Port::request_monitor`](crate::Port::request_monitor) can be called on
    /// this port.
    ///
    /// Precisely what this means is dependent on the client. A typical result of it being called
    /// with `true` is that data that would be available from an output port (with
    /// [`is_physical`](Self::is_physical) set) is sent to a physical output connector as well, so
    /// that it can be heard/seen/whatever.
    ///
    /// Clients that do not control physical interfaces should never create ports with this bit
    /// set.
    #[inline]
    #[doc(alias = "JackPortCanMonitor")]
    pub fn can_monitor(&self) -> bool {
        (self.0 & sys::JackPortFlags_JackPortCanMonitor) != 0
    }
    /// Returns `true` if the port is a terminal end for data flow.
    ///
    /// Depending on the port mode, this means:
    ///
    /// - [`is_input`](Self::is_input) - The data received by the port will not be passed on or
    ///   made available at any other port.
    /// - [`is_output`](Self::is_output) - The data available at the port does not originate from
    ///   any other port.
    ///
    /// Audio synthesizers, I/O hardware interface clients, HDR systems are examples of clients
    /// that would set this flag for their ports.
    #[doc(alias = "JackPortIsTerminal")]
    #[inline]
    pub fn is_terminal(&self) -> bool {
        (self.0 & sys::JackPortFlags_JackPortIsTerminal) != 0
    }
    /// Returns the mode corresponding to the flags.
    #[inline]
    pub fn mode(&self) -> PortMode {
        if self.is_input() {
            PortMode::Input
        } else if self.is_output() {
            PortMode::Output
        } else {
            panic!("INPUT or OUTPUT not present in flags")
        }
    }
}

impl From<PortFlags> for sys::JackPortFlags {
    #[inline]
    fn from(flags: PortFlags) -> Self {
        flags.0
    }
}

/// An ID representing common port types.
///
/// These bindings only support common port types implemented by the default jackd2 server and
/// by pipewire-jack. Alternate JACK servers implementing custom types are not supported.
///
/// If a client needs to implement a custom type, the
/// [`ProcessPort::as_ptr`](crate::ProcessPort::as_ptr) and
/// [`ProcessScope::library`](crate::ProcessScope::library) methods are provided to give access to the C FFI.
#[doc(alias = "jack_port_type_id_t")]
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub enum PortType {
    /// 32-bit float mono audio.
    Audio,
    /// 8-bit raw MIDI.
    Midi,
}

impl PortType {
    #[inline]
    pub(crate) fn as_cstr(&self) -> &'static CStr {
        let ty = match self {
            Self::Audio => sys::JACK_DEFAULT_AUDIO_TYPE.as_slice(),
            Self::Midi => sys::JACK_DEFAULT_MIDI_TYPE.as_slice(),
        };
        unsafe { CStr::from_bytes_with_nul_unchecked(ty) }
    }
    /// Returns a human-readable string describing the port type.
    #[inline]
    pub fn as_str(&self) -> &'static str {
        self.as_cstr().to_str().unwrap()
    }
}

/// The input/output mode of the port.
///
/// Corresponds to the same flags in [`PortFlags`]. Only used by [`PortInfo`](crate::PortInfo) when
/// defining ports, to ensure only one mode is selected.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub enum PortMode {
    /// The port can receive data.
    Input,
    /// Data can be read from the port.
    Output,
}

impl PortMode {
    #[inline]
    pub(crate) fn flags(&self) -> sys::JackPortFlags {
        match self {
            Self::Input => sys::JackPortFlags_JackPortIsInput,
            Self::Output => sys::JackPortFlags_JackPortIsOutput,
        }
    }
}

/// A min/max range for port latency.
#[doc(alias = "jack_latency_range_t")]
#[doc(alias = "_jack_latency_range")]
#[derive(Debug)]
pub struct LatencyRange {
    /// Minimum latency.
    pub min: Frames,
    /// Maximum latency.
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
    #[inline]
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { library().jack_free(self.0 as *mut _) }
        }
    }
}
