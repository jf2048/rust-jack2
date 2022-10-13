use atb::AtomicTripleBuffer;
use std::{
    cell::{Cell, RefCell, UnsafeCell},
    collections::HashMap,
    ffi::{CStr, CString},
    num::NonZeroU64,
    ptr::NonNull,
    rc::Rc,
    sync::Arc,
    time::Duration,
};

use crate::{
    callbacks::*, sys, Error, Failure, Frames, Port, PortFlags, PortId, PortList, PortMode,
    PortPtr, PortType, ProcessPortOuter, ProcessPorts, Time, Transport, Uuid,
};

#[derive(Clone, Debug)]
pub(crate) struct ClientHandle {
    pub lib: &'static sys::Jack,
    pub client: NonNull<sys::jack_client_t>,
}

impl ClientHandle {
    #[inline]
    pub(crate) fn as_ptr(&self) -> *mut sys::jack_client_t {
        self.client.as_ptr()
    }
}

bitflags::bitflags! {
    /// Options for starting a new client.
    ///
    /// See also [`ClientBuilder::flags`].
    #[doc(alias = "JackOptions")]
    #[doc(alias = "jack_options_t")]
    #[derive(Default)]
    pub struct ClientOptions: sys::JackOptions {
        /// Do not automatically start the JACK server when it is not already running.
        ///
        /// This option is always selected if `JACK_NO_START_SERVER` is defined in the calling
        /// process environment.
        #[doc(alias = "JackNoStartServer")]
        const NO_START_SERVER = sys::JackOptions_JackNoStartServer;
        /// Use the exact client name requested.  Otherwise, JACK automatically generates a unique
        /// one, if needed.
        #[doc(alias = "JackUseExactName")]
        const USE_EXACT_NAME  = sys::JackOptions_JackUseExactName;
    }
}

struct ThreadGuard(std::thread::ThreadId);

impl ThreadGuard {
    fn new() -> Self {
        Self(std::thread::current().id())
    }
    fn check(&self, msg: &str) {
        if std::thread::current().id() != self.0 {
            panic!("{}", msg);
        }
    }
}

/// Client interaction with a Rust async runtime.
///
/// This trait must be implemented by users of [`Client`]. Sample implementations of this trait are
/// provided by the `tokio` and `glib` feature flags.
/// repository for sample implementations.
pub trait MainThreadContext {
    /// Spawn a future on the current thread.
    ///
    /// The semantics of this method should match
    /// [`tokio::task::spawn_local`](https://docs.rs/tokio/1/tokio/task/fn.spawn_local.html).
    fn spawn_local<F: std::future::Future<Output = ()> + 'static>(&self, fut: F);
    /// A stream type that can be used to run a task at specified intervals.
    ///
    /// The semantics of this stream should match
    /// [`tokio_stream::wrappers::IntervalStream`](https://docs.rs/tokio-stream/0.1.10/tokio_stream/wrappers/struct.IntervalStream.html),
    /// but should always return `()`.
    type IntervalStream: futures_core::Stream<Item = ()> + 'static;
    /// Create a [`Stream`](futures_core::Stream) that yields with intervals of `period`.
    ///
    /// The semantics of this method should match
    /// [`tokio::time::interval`](https://docs.rs/tokio/1/tokio/time/fn.interval.html).
    fn interval(&self, period: Duration) -> Self::IntervalStream;
}

/// A builder for constructing [`Client`].
#[derive(Debug)]
#[must_use]
pub struct ClientBuilder<Context>
where
    Context: MainThreadContext,
{
    name: String,
    flags: sys::jack_options_t,
    server_name: Option<CString>,
    context: Context,
}

impl<Context> ClientBuilder<Context>
where
    Context: MainThreadContext,
{
    /// Create a builder for opening an external client session with a JACK server.
    ///
    /// `client_name` is a string of at most [`crate::client_name_size`] characters. The name scope
    /// is local to each server. Unless forbidden by the [`ClientOptions::USE_EXACT_NAME`] flag,
    /// the server will modify this name to create a unique variant, if needed.
    ///
    /// When using this function, the type of `Context` must be manually specified somewhere. See
    /// [`Self::with_context`] for a constructor that allows passing a context object.
    #[doc(alias = "jack_client_open")]
    #[inline]
    pub fn new(client_name: &str) -> Self
    where
        Context: Default,
    {
        Self {
            name: client_name.to_owned(),
            flags: sys::JackOptions_JackNullOption,
            server_name: None,
            context: Default::default(),
        }
    }
    /// Create a builder for opening an external client session with a JACK server, using a custom
    /// thread context.
    ///
    /// See [`Self::new`].
    #[inline]
    pub fn with_context(client_name: &str, context: Context) -> Self {
        Self {
            name: client_name.to_owned(),
            flags: sys::JackOptions_JackNullOption,
            server_name: None,
            context,
        }
    }
    /// Sets flags for connecting to the JACK server.
    #[inline]
    pub fn flags(mut self, flags: ClientOptions) -> Self {
        self.flags |= flags.bits();
        self
    }
    /// Selects from among several possible concurrent server instances, specified by
    /// `server_name`. Server names are unique to each user. If unspecified, use "default"
    /// unless `JACK_DEFAULT_SERVER` is defined in the process environment.
    #[doc(alias = "JackServerName")]
    #[inline]
    pub fn server_name(mut self, server_name: impl Into<CString>) -> Self {
        self.server_name = Some(server_name.into());
        self.flags |= sys::JackOptions_JackServerName;
        self
    }
    /// Create the JACK client.
    ///
    /// Returns `Err` if connecting to the server failed or if the JACK shared library failed to
    /// load.
    #[doc(alias = "jack_client_open")]
    pub fn build<PortData>(self) -> crate::Result<(InactiveClient<Context, PortData>, Status)>
    where
        PortData: Send + 'static,
    {
        let lib = unsafe { sys::weak_library()? };
        static JACK_LOG: std::sync::Once = std::sync::Once::new();
        JACK_LOG.call_once(|| {
            unsafe extern "C" fn error_handler(msg: *const std::os::raw::c_char) {
                match std::ffi::CStr::from_ptr(msg).to_str() {
                    Ok(msg) => log::error!("{}", msg),
                    Err(err) => log::error!("failed to parse JACK error: {:?}", err),
                }
            }
            unsafe extern "C" fn info_handler(msg: *const std::os::raw::c_char) {
                match std::ffi::CStr::from_ptr(msg).to_str() {
                    Ok(msg) => log::info!("{}", msg),
                    Err(err) => log::error!("failed to parse JACK error: {:?}", err),
                }
            }

            unsafe {
                lib.jack_set_error_function(Some(error_handler));
                lib.jack_set_info_function(Some(info_handler));
            }
        });

        let Self {
            name,
            flags,
            server_name,
            context,
            ..
        } = self;

        let name = CString::new(name)?;

        let mut status = 0;
        let client = unsafe {
            if let Some(server_name) = server_name {
                (lib.jack_client_open.as_ref()?)(
                    name.as_ptr(),
                    flags,
                    &mut status,
                    server_name.as_ptr(),
                )
            } else {
                (lib.jack_client_open.as_ref()?)(name.as_ptr(), flags, &mut status)
            }
        };
        let client = NonNull::new(client).ok_or(Failure(status))?;
        let client = ClientHandle { lib, client };

        let process_ports = Arc::new(AtomicTripleBuffer::default());

        Ok((
            InactiveClient(Client {
                client,
                ports: Default::default(),
                process_ports,
                context,
                pending_port_gc: Default::default(),
            }),
            Status(status),
        ))
    }
}

bitflags::bitflags! {
    /// Set of flags to use when creating ports.
    #[doc(alias = "JackPortFlags")]
    #[derive(Default)]
    pub struct PortCreateFlags: sys::JackPortFlags {
        /// Port corresponds to some kind of physical I/O connector.
        #[doc(alias = "JackPortIsPhysical")]
        const PHYSICAL    = sys::JackPortFlags_JackPortIsPhysical;
        /// Enables calling [`Port::request_monitor`](crate::Port::request_monitor) on this port.
        ///
        /// Precisely what this means is dependent on the client. A typical result of it being
        /// called with `true` is that data that would be available from an output port (with
        /// [`PHYSICAL`](Self::PHYSICAL) set) is sent to a physical output connector as well, so
        /// that it can be heard/seen/whatever.
        ///
        /// Clients that do not control physical interfaces should never create ports with this bit
        /// set.
        #[doc(alias = "JackPortCanMonitor")]
        const CAN_MONITOR = sys::JackPortFlags_JackPortCanMonitor;
        /// Port is a terminal end for data flow.
        ///
        /// Depending on the port mode, this means:
        ///
        /// - [`Input`](PortMode::Input) - The data received by the port will not be passed on or
        ///   made available at any other port.
        /// - [`Output`](PortMode::Output) - The data available at the port does not originate from
        ///   any other port.
        ///
        /// Audio synthesizers, I/O hardware interface clients, HDR systems are examples of clients
        /// that would set this flag for their ports.
        #[doc(alias = "JackPortIsTerminal")]
        const TERMINAL = sys::JackPortFlags_JackPortIsTerminal;
    }
}

/// Status returned from JACK client creation.
#[doc(alias = "JackStatus")]
#[doc(alias = "jack_status_t")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(transparent)]
pub struct Status(sys::jack_status_t);

impl Status {
    /// Returns `true` if the desired client was not unique.
    ///
    /// If `false` is returned, the name was modified by appending a dash and a two-digit number in
    /// the range "-01" to "-99".  The [`Client::name`] method will return the exact string that
    /// was used. If the specified `client_name` plus these extra characters would be too long, the
    /// open fails instead.
    #[doc(alias = "JackNameNotUnique")]
    #[inline]
    pub fn has_exact_name(&self) -> bool {
        (self.0 & sys::JackStatus_JackNameNotUnique) == 0
    }
    /// Returns `true` if the JACK server was started as a result of this operation.
    ///
    /// If `false` was returned, the server was running already. In either case the caller is now
    /// connected to the JACK server, so there is no race condition. When the server shuts down,
    /// the client will find out.
    #[doc(alias = "JackServerStarted")]
    #[inline]
    pub fn server_started(&self) -> bool {
        (self.0 & sys::JackStatus_JackServerStarted) != 0
    }
}

impl From<Status> for sys::jack_status_t {
    #[inline]
    fn from(status: Status) -> Self {
        status.0
    }
}

bitflags::bitflags! {
    /// Flags to use when activating a JACK client.
    ///
    /// Used in [`InactiveClient::activate`].
    #[derive(Default)]
    pub struct ActivateFlags: u32 {
        /// Tell the JACK server to call the latency callback whenever it is necessary to recompute
        /// the latencies for some or all JACK ports.
        ///
        /// Using this flag enables [`Notification::Latency`] messages (and
        /// [`NotificationHandler::latency`] method calls) to be received. This flag must be
        /// specified when calling [`InactiveClient::activate`] to receive that notification. See
        /// the documentation of those items for more details.
        ///
        /// If this flag is not used, the JACK library automatically includes a default latency
        /// callback for all clients. Using this flag will disable the default latency callback,
        /// so clients should not use this if they do not handle latency notifications.
        #[doc(alias = "jack_set_latency_callback")]
        const LATENCY        = 1 << 0;
        /// Register as a slow-sync client.
        ///
        /// This flag must be used to start receiving [`ProcessHandler::sync`]. As an alternative
        /// to this flag, [`ActiveClient::register_sync_callback`] can be called after activating
        /// the client.
        ///
        /// Slow-sync clients cannot respond immediately to transport position changes.
        /// [`ProcessHandler::sync`] will be invoked at the first available opportunity after its
        /// registration is complete. If the client is currently active this will be the following
        /// process cycle, otherwise it will be the first cycle after calling
        /// [`InactiveClient::activate`]. After that, it runs according to the rules specified on
        /// [`ProcessHandler::sync`]. Clients that don't set a sync callback are assumed to be
        /// ready immediately any time the transport wants to start.
        #[doc(alias = "jack_set_sync_callback")]
        const SYNC           = 1 << 1;
        /// Register as timebase master for the JACK subsystem.
        ///
        /// This flag must be used to start receiving [`ProcessHandler::timebase`]. As an
        /// alternative to this flag, [`ActiveClient::acquire_timebase`] can be called after
        /// activating the client.
        ///
        /// The timebase master registers a callback that updates extended position information such as
        /// beats or timecode whenever necessary. Without this extended information, there is no need
        /// for this method.
        ///
        /// There is never more than one master at a time. When a new client takes over, the former
        /// [`ProcessHandler::timebase`] is no longer called. Taking over the timebase may be done
        /// also be done conditionally by using the [`Self::TIMEBASE_FORCE`] flag.
        #[doc(alias = "jack_set_timebase_callback")]
        const TIMEBASE       = 1 << 2;
        /// Register as timebase master for the JACK subsystem, failing activation if a timebase
        /// master already was registered.
        ///
        /// Otherwise, this flag is the same as [`Self::TIMEBASE`].
        #[doc(alias = "jack_set_timebase_callback")]
        const TIMEBASE_FORCE = (1 << 3) | Self::TIMEBASE.bits;
    }
}

/// An opaque JACK client handle.
///
/// This structure contains common methods available to both [`InactiveClient`] and
/// [`ActiveClient`].
#[doc(alias = "jack_client_t")]
#[derive(Debug)]
pub struct Client<Context, PortData = ()>
where
    Context: MainThreadContext,
    PortData: Send + 'static,
{
    client: ClientHandle,
    ports: RefCell<HashMap<OwnedPortUuid, ProcessPortOuter<PortData>>>,
    process_ports: Arc<AtomicTripleBuffer<ProcessPorts<PortData>>>,
    context: Context,
    pending_port_gc: Rc<Cell<bool>>,
}

impl<Context, PortData> Client<Context, PortData>
where
    Context: MainThreadContext,
    PortData: Send + 'static,
{
    /// Change the buffer size used in [`ProcessHandler::process`].
    ///
    /// This operation stops the JACK engine process cycle, then calls all registered
    /// [`ProcessHandler::buffer_size`] methods before restarting the process cycle. This will
    /// cause a gap in the audio flow, so it should only be done at appropriate stopping points.
    ///
    /// `nframes` is the new buffer size. Must be a power of two.
    #[doc(alias = "jack_set_buffer_size")]
    pub fn set_buffer_size(&self, nframes: u32) -> crate::Result<()> {
        Error::check_ret(unsafe {
            self.client
                .lib
                .jack_set_buffer_size(self.client.as_ptr(), nframes)
        })
    }
    /// Return the current buffer size.
    pub fn buffer_size(&self) -> u32 {
        unsafe { self.client.lib.jack_get_buffer_size(self.client.as_ptr()) }
    }
    /// Returns the sample rate of the JACK system, as set by the user when the server was started.
    #[doc(alias = "jack_get_sample_rate")]
    pub fn sample_rate(&self) -> u32 {
        unsafe { self.client.lib.jack_get_sample_rate(self.client.as_ptr()) }
    }
    /// Returns the current CPU load estimated by JACK.
    ///
    /// This is a running average of the time it takes to execute a full process cycle for all
    /// clients as a percentage of the real time available per cycle determined by the buffer size
    /// and sample rate.
    #[doc(alias = "jack_cpu_load")]
    pub fn cpu_load(&self) -> f32 {
        unsafe { self.client.lib.jack_cpu_load(self.client.as_ptr()) }
    }
    /// Returns the length of a frame as a [`Duration`].
    ///
    /// Equivalent to
    /// <code>[buffer_size](Self::buffer_size) / [sample_rate](Self::sample_rate)</code>.
    pub fn frame_duration(&self) -> Duration {
        let secs = self.buffer_size() as f64 / self.sample_rate() as f64;
        std::time::Duration::from_secs_f64(secs)
    }
    /// Returns the estimated current time in frames.
    ///
    /// The return value can be compared with the value of
    /// [`ProcessScope::last_frame_time`](crate::ProcessScope::last_frame_time) to relate time in
    /// other threads to JACK time.
    #[doc(alias = "jack_frame_time")]
    pub fn frame_time(&self) -> Frames {
        Frames(unsafe { self.client.lib.jack_frame_time(self.client.as_ptr()) })
    }
    /// Returns the estimated time in frames that has passed since the JACK server began the current process cycle.
    #[doc(alias = "jack_frames_since_cycle_start")]
    pub fn frames_since_cycle_start(&self) -> Frames {
        Frames(unsafe {
            self.client
                .lib
                .jack_frames_since_cycle_start(self.client.as_ptr())
        })
    }
    /// Returns the estimated time in microseconds of the specified frame time.
    #[doc(alias = "jack_frames_to_time")]
    pub fn frames_to_time(&self, frames: Frames) -> Time {
        Time(unsafe {
            self.client
                .lib
                .jack_frames_to_time(self.client.as_ptr(), frames.0)
        })
    }
    /// Returns the estimated time in frames for the specified system time.
    #[doc(alias = "jack_time_to_frames")]
    pub fn time_to_frames(&self, time: Time) -> Frames {
        Frames(unsafe {
            self.client
                .lib
                .jack_time_to_frames(self.client.as_ptr(), time.0)
        })
    }
    /// Start/Stop JACK's "freewheel" mode.
    ///
    /// When in "freewheel" mode, JACK no longer waits for any external event to begin the start of
    /// the next process cycle.
    ///
    /// As a result, freewheel mode causes "faster than real-time" execution of a JACK graph. If
    /// possessed, real-time scheduling is dropped when entering freewheel mode, and if appropriate
    /// it is reacquired when stopping.
    #[doc(alias = "jack_set_freewheel")]
    pub fn set_freewheel(&self, enabled: bool) -> crate::Result<()> {
        Error::check_ret(unsafe {
            self.client
                .lib
                .jack_set_freewheel(self.client.as_ptr(), enabled as _)
        })
    }
    /// Create new ports for the client. A port is an object used for moving data of any type in or
    /// out of the client. Ports may be connected in various ways.
    ///
    /// This method takes an iterator of [`PortInfo`]s defining the ports to be registered. Any
    /// ports registered successfully will be available in the next process cycle. Calls to this
    /// method register all given ports atomically; callers should try to register as many ports
    /// as they can in each call, for example when creating multiple ports for stereo/surround.
    ///
    /// Returns a [`Vec`] of [`OwnedPortUuid`] representing each of the registered ports, in the
    /// same order as the passed iterator. The `Vec` will always be the same size as the iterator,
    /// even if some ports fail to register. Ports that fail to register will have `None` at the
    /// corresponding index.
    #[doc(alias = "jack_port_register")]
    pub fn register_ports<'p>(
        &self,
        ports: impl IntoIterator<Item = PortInfo<'p, PortData>>,
    ) -> Vec<Result<OwnedPortUuid, PortRegisterError>> {
        let mut ports = ports
            .into_iter()
            .enumerate()
            .map(
                |(
                    i,
                    PortInfo {
                        name,
                        type_,
                        mode,
                        flags,
                        data,
                    },
                )| {
                    let port_name = CString::new(name).map_err(|_| PortRegisterError(i))?;
                    let flags = mode.flags() | flags.bits();

                    unsafe {
                        let port = self.client.lib.jack_port_register(
                            self.client.as_ptr(),
                            port_name.as_ptr(),
                            type_.as_cstr().as_ptr(),
                            flags.into(),
                            0,
                        );
                        let ptr = PortPtr::new(self.client.clone(), port, PortFlags(flags), type_)
                            .ok_or(PortRegisterError(i))?;
                        let port = OwnedPortUuid::new(&self.client, port).unwrap();
                        Ok((port, ptr, data))
                    }
                },
            )
            .peekable();
        if ports.peek().is_some() {
            let ports = ports
                .map(|port| {
                    port.map(|(port, ptr, data)| {
                        self.ports
                            .borrow_mut()
                            .insert(port, ProcessPortOuter::new(ptr, data));
                        port
                    })
                })
                .collect();
            let mut bufs = self.process_ports.back_buffers().unwrap();
            bufs.back_mut().ports = self.ports.borrow().clone();
            bufs.swap();
            ports
        } else {
            Vec::new()
        }
    }
    /// Remove ports from the client, disconnecting any existing connections.
    ///
    /// The contents of `ports` can be any port that was previously returned by a call to
    /// [`Self::register_ports`] on this client. If called while a process cycle is running, the
    /// ports will still be available until the next process cycle.
    #[doc(alias = "jack_port_unregister")]
    pub fn unregister_ports(&self, ports: impl IntoIterator<Item = OwnedPortUuid>) {
        let mut ports = ports.into_iter().peekable();
        if ports.peek().is_none() {
            return;
        }
        let mut port_map = self.ports.borrow_mut();
        for port in ports {
            port_map.remove(&port);
        }
        let mut bufs = self.process_ports.back_buffers().unwrap();
        bufs.back_mut().ports = port_map.clone();
        bufs.swap();
        drop(port_map);
        if !self.pending_port_gc.get() {
            self.pending_port_gc.set(true);
            let mut interval = Box::pin(self.context.interval(self.frame_duration()));
            let ports = Arc::downgrade(&self.process_ports);
            let guard = ThreadGuard::new();
            let pending_port_gc = Rc::downgrade(&self.pending_port_gc);
            self.context.spawn_local(async move {
                use futures_util::StreamExt;
                guard
                    .check("Expected spawn_local to spawn future on the same thread as the Client");
                loop {
                    interval.next().await;
                    let ports = match ports.upgrade() {
                        Some(ports) => ports,
                        None => break,
                    };
                    let mut bufs = ports.back_buffers().unwrap();
                    bufs.back_mut().ports.clear();
                    if let Some(pending) = bufs.pending_mut() {
                        pending.ports.clear();
                        break;
                    }
                }
                if let Some(pending_port_gc) = pending_port_gc.upgrade() {
                    pending_port_gc.set(false);
                }
            });
        }
    }
    /// Searches the JACK server for a matching list of ports.
    ///
    /// # Parameters
    ///
    /// - `port_name_pattern` - A regular expression used to select ports by name. If `None` or of
    ///   zero length, no selection based on name will be carried out.
    /// - `type_name_pattern` - A regular expression used to select ports by type. If `None` or of
    ///   zero length, no selection based on type will be carried out.
    /// - `flags ` - A value used to select ports by their flags. If empty, no selection based on
    ///   flags will be carried out.
    ///
    /// Returns a list of full port names that match the specified arguments.
    #[doc(alias = "jack_get_ports")]
    pub fn ports(
        &self,
        port_name_pattern: Option<impl AsRef<CStr>>,
        type_name_pattern: Option<impl AsRef<CStr>>,
        flags: PortFlags,
    ) -> Vec<CString> {
        PortList(unsafe {
            self.client.lib.jack_get_ports(
                self.client.client.as_ptr(),
                port_name_pattern
                    .map(|p| p.as_ref().as_ptr())
                    .unwrap_or(std::ptr::null()),
                type_name_pattern
                    .map(|p| p.as_ref().as_ptr())
                    .unwrap_or(std::ptr::null()),
                flags.0 as _,
            )
        })
        .to_vec()
    }
    /// Returns a [`Port`] object for the owned port UUID.
    ///
    /// Returns `None` if the port does not exist.
    pub fn port_by_owned_uuid(&self, port: OwnedPortUuid) -> Option<Port> {
        self.ports
            .borrow()
            .get(&port)
            .and_then(|p| Port::new(&self.client, p.0.ptr.port.as_ptr()))
    }
    /// Returns a [`Port`] object for the port with the given full name.
    ///
    /// Returns `None` if the port does not exist.
    #[doc(alias = "jack_port_by_name")]
    pub fn port_by_name(&self, name: impl AsRef<CStr>) -> Option<Port> {
        unsafe {
            Port::new(
                &self.client,
                self.client
                    .lib
                    .jack_port_by_name(self.client.as_ptr(), name.as_ref().as_ptr()),
            )
        }
    }
    /// Returns a [`Port`] object for the port with a given ID.
    ///
    /// Returns `None` if the port does not exist.
    #[doc(alias = "jack_port_by_id")]
    pub fn port_by_id(&self, id: PortId) -> Option<Port> {
        unsafe {
            Port::new(
                &self.client,
                self.client
                    .lib
                    .jack_port_by_id(self.client.as_ptr(), id.0.get()),
            )
        }
    }
    /// Returns `true` if the [`Port`] object belongs to this client.
    #[doc(alias = "jack_port_is_mine")]
    pub fn port_is_mine(&self, port: Port) -> bool {
        (unsafe {
            self.client
                .lib
                .jack_port_is_mine(self.client.client.as_ptr(), port.as_ptr().as_ptr())
        }) == 1
    }
    /// Returns a list of full port names to which `port` is connected.
    ///
    /// This differs from [`Port::connections`] in that you need not be the owner of the port to
    /// get information about its connections.
    #[doc(alias = "jack_port_get_all_connections")]
    pub fn port_get_all_connections(&self, port: Port) -> Vec<CString> {
        PortList(unsafe {
            self.client
                .lib
                .jack_port_get_all_connections(self.client.client.as_ptr(), port.as_ptr().as_ptr())
        })
        .to_vec()
    }
    /// Modify a port's short name.
    ///
    /// If the resulting full name (including the `client_name:` prefix) is longer than
    /// [`crate::port_name_size`], it will be truncated.
    #[doc(alias = "jack_port_rename")]
    pub fn port_rename(
        &self,
        port: OwnedPortUuid,
        port_name: impl AsRef<CStr>,
    ) -> crate::Result<()> {
        let port = self
            .ports
            .borrow()
            .get(&port)
            .map(|p| p.0.ptr.port)
            .ok_or_else(Error::invalid_option)?;
        Error::check_ret(unsafe {
            self.client.lib.jack_port_rename(
                self.client.as_ptr(),
                port.as_ptr(),
                port_name.as_ref().as_ptr(),
            )
        })
    }
    /// Toggle input monitoring on a port.
    ///
    /// If [`PortFlags::can_monitor`] is set for the port fully named `port_name`, turn input
    /// monitoring on or off. Otherwise, do nothing.
    #[doc(alias = "jack_port_request_monitor_by_name")]
    pub fn port_request_monitor_by_name(
        &self,
        port_name: impl AsRef<CStr>,
        onoff: bool,
    ) -> crate::Result<()> {
        Error::check_ret(unsafe {
            self.client.lib.jack_port_request_monitor_by_name(
                self.client.as_ptr(),
                port_name.as_ref().as_ptr(),
                onoff as _,
            )
        })
    }
    /// Establish a connection between two ports.
    ///
    /// When a connection exists, data written to the source port will be available to be read at
    /// the destination port. The port types must be identical.
    ///
    /// The [`PortFlags`] of `source_port` must return `true` for [`PortFlags::is_output`]. The
    /// [`PortFlags`] of `destination_port` must return `true` for [`PortFlags::is_input`].
    ///
    /// Returns `Err` if the connection is already made.
    #[doc(alias = "jack_connect")]
    pub fn connect(
        &self,
        source_port: impl AsRef<CStr>,
        destination_port: impl AsRef<CStr>,
    ) -> crate::Result<()> {
        Error::check_ret(unsafe {
            self.client.lib.jack_connect(
                self.client.as_ptr(),
                source_port.as_ref().as_ptr(),
                destination_port.as_ref().as_ptr(),
            )
        })
    }
    /// Remove a connection between two ports.
    ///
    /// The port types must be identical.
    ///
    /// The [`PortFlags`] of `source_port` must return `true` for [`PortFlags::is_output`]. The
    /// [`PortFlags`] of `destination_port` must return `true` for [`PortFlags::is_input`].
    #[doc(alias = "jack_disconnect")]
    pub fn disconnect(
        &self,
        source_port: impl AsRef<CStr>,
        destination_port: impl AsRef<CStr>,
    ) -> crate::Result<()> {
        Error::check_ret(unsafe {
            self.client.lib.jack_disconnect(
                self.client.as_ptr(),
                source_port.as_ref().as_ptr(),
                destination_port.as_ref().as_ptr(),
            )
        })
    }
    /// Perform the same function as [`Self::disconnect`] using port handles rather than names.
    ///
    /// This avoids the name lookup inherent in the name-based version. Clients connecting their
    /// own ports are likely to use this method, while generic connection clients (e.g.
    /// patchbays) would use [`Self::disconnect`].
    #[doc(alias = "jack_port_disconnect")]
    pub fn port_disconnect(&self, port: OwnedPortUuid) -> crate::Result<()> {
        let port = self
            .ports
            .borrow()
            .get(&port)
            .map(|p| p.0.ptr.port)
            .ok_or_else(Error::invalid_option)?;
        Error::check_ret(unsafe {
            self.client
                .lib
                .jack_port_disconnect(self.client.as_ptr(), port.as_ptr())
        })
    }
    /// Request a complete recomputation of all port latencies.
    ///
    /// This can be called by a client that has just changed the internal latency of its port using
    /// [`Port::set_latency_range`] and wants to ensure that all signal pathways in the graph are
    /// updated with respect to the values that will be returned by [`Port::latency_range`]. It
    /// allows a client to change multiple port latencies without triggering a recompute for each
    /// change.
    #[doc(alias = "jack_recompute_total_latencies")]
    pub fn recompute_total_latencies(&self) -> crate::Result<()> {
        Error::check_ret(unsafe {
            self.client
                .lib
                .jack_recompute_total_latencies(self.client.as_ptr())
        })
    }
    /// Set the timeout value for slow-sync clients.
    ///
    /// This timeout prevents unresponsive slow-sync clients from completely halting the transport
    /// mechanism. The default is two seconds. When the timeout expires, the transport starts
    /// rolling, even if some slow-sync clients are still unready. The [`ProcessHandler::sync`]
    /// methods of these clients continue being invoked, giving them a chance to catch up.
    ///
    /// # Parameters
    ///
    /// - `timeout` - Delay (in microseconds) before the timeout expires.
    #[doc(alias = "jack_set_sync_timeout")]
    pub fn set_sync_timeout(&self, timeout: Time) -> crate::Result<()> {
        Error::check_ret(unsafe {
            self.client
                .lib
                .jack_set_sync_timeout(self.client.as_ptr(), timeout.as_micros())
        })
    }
    /// Returns a object that can used to control the JACK transport.
    #[inline]
    pub fn transport(&self) -> Transport {
        Transport::new(&self.client)
    }
    /// Returns actual client name.
    ///
    /// This is useful when [`ClientOptions::USE_EXACT_NAME`] is not specified with
    /// [`ClientBuilder::flags`] and [`Status::has_exact_name`] returns `false`. In that case, the
    /// actual name will differ from the `client_name` requested.
    #[doc(alias = "jack_get_client_name")]
    pub fn name(&self) -> CString {
        unsafe {
            let ptr = self.client.lib.jack_get_client_name(self.client.as_ptr());
            crate::JackStr::from_raw_unchecked(ptr).as_cstring()
        }
    }
    /// Get the assigned UUID for the current client.
    #[doc(alias = "jack_client_get_uuid")]
    pub fn uuid(&self) -> Uuid {
        unsafe {
            Uuid::from_raw(self.client.lib.jack_client_get_uuid(self.client.as_ptr())).unwrap()
        }
    }
    /// Get the session ID for a client name.
    ///
    /// The session manager needs this to reassociate a client name to the session ID.
    #[doc(alias = "jack_get_uuid_for_client_name")]
    pub fn uuid_for_client_name(&self, name: impl AsRef<CStr>) -> Option<Uuid> {
        unsafe {
            Uuid::from_raw(
                self.client
                    .lib
                    .jack_get_uuid_for_client_name(self.client.as_ptr(), name.as_ref().as_ptr()),
            )
        }
    }
    /// Get the client name for a session ID.
    ///
    /// In order to snapshot the graph connections, the session manager needs to map session IDs to
    /// client names.
    #[doc(alias = "jack_get_client_name_by_uuid")]
    pub fn client_name_by_uuid(&self, uuid: Uuid) -> Option<CString> {
        let uuid = CString::new(uuid.to_string()).unwrap();
        unsafe {
            let ptr = self
                .client
                .lib
                .jack_get_client_name_by_uuid(self.client.as_ptr(), uuid.as_ptr());
            Some(crate::JackStr(NonNull::new(ptr)?).as_cstring())
        }
    }
    /// Set a property on subject.
    ///
    /// See [`Property`](crate::Property) for rules about subject and key.
    ///
    /// # Parameters
    ///
    /// * `subject` - The subject to set the property on.
    /// * `key` - The key of the property.
    /// * `value` - The value of the property.
    /// * `type_` - The type of the property. See [`Property::type_`](crate::Property::type_).
    #[doc(alias = "jack_set_property")]
    pub fn set_property(
        &self,
        subject: Uuid,
        key: impl AsRef<CStr>,
        value: impl AsRef<CStr>,
        type_: Option<impl AsRef<CStr>>,
    ) -> crate::Result<()> {
        Error::check_ret(unsafe {
            self.client.lib.jack_set_property(
                self.client.as_ptr(),
                subject.0.get(),
                key.as_ref().as_ptr(),
                value.as_ref().as_ptr(),
                type_
                    .map(|t| t.as_ref().as_ptr())
                    .unwrap_or_else(std::ptr::null),
            )
        })
    }
    /// Remove a single metadata property on a subject.
    ///
    /// # Parameters
    ///
    /// - `subject` - The subject to remove the property from.
    /// - `key` - The key of the property to be removed.
    #[doc(alias = "jack_remove_property")]
    pub fn remove_property(&self, subject: Uuid, key: impl AsRef<CStr>) -> crate::Result<()> {
        Error::check_ret(unsafe {
            self.client.lib.jack_remove_property(
                self.client.as_ptr(),
                subject.0.get(),
                key.as_ref().as_ptr(),
            )
        })
    }
    /// Remove all metadata properties on a subject.
    ///
    /// Returns a count of the number of properties removed, or `Err`.
    #[doc(alias = "jack_remove_properties")]
    pub fn remove_properties(&self, subject: Uuid) -> crate::Result<u32> {
        let count = unsafe {
            self.client
                .lib
                .jack_remove_properties(self.client.as_ptr(), subject.0.get())
        };
        Error::check_ret(count)?;
        Ok(count as u32)
    }
    /// Remove all metadata properties.
    ///
    /// ⚠️ WARNING ⚠️
    ///
    /// This deletes all metadata managed by a running JACK server. Data lost cannot be recovered
    /// (though it can be recreated by new calls to [`Self::set_property`]).
    #[doc(alias = "jack_remove_all_properties")]
    pub fn remove_all_properties(&self) -> crate::Result<()> {
        Error::check_ret(unsafe {
            self.client
                .lib
                .jack_remove_all_properties(self.client.as_ptr())
        })
    }
    /// Returns the maximum delay reported by the backend since startup or reset.
    ///
    /// When compared to the period size in usecs, this can be used to estimate the ideal period
    /// size for a given setup.
    #[doc(alias = "jack_get_max_delayed_usecs")]
    pub fn max_delayed_usecs(&self) -> f32 {
        unsafe {
            self.client
                .lib
                .jack_get_max_delayed_usecs(self.client.as_ptr())
        }
    }
    /// Returns the delay in microseconds due to the most recent XRUN occurrence.
    ///
    /// This probably only makes sense when called from [`NotificationHandler::xrun`].
    #[doc(alias = "jack_get_xrun_delayed_usecs")]
    pub fn xrun_delayed_usecs(&self) -> f32 {
        unsafe {
            self.client
                .lib
                .jack_get_xrun_delayed_usecs(self.client.as_ptr())
        }
    }
    /// Reset the maximum delay counter.
    ///
    /// This would be useful to estimate the effect that a change to the configuration of a running
    /// system (e.g. toggling kernel preemption) has on the delay experienced by JACK, without
    /// having to restart the JACK engine.
    #[doc(alias = "jack_reset_max_delayed_usecs")]
    pub fn reset_max_delayed_usecs(&self) {
        unsafe {
            self.client
                .lib
                .jack_reset_max_delayed_usecs(self.client.as_ptr());
        }
    }
    /// Returns the C pointer corresponding to this client.
    #[inline]
    pub fn as_ptr(&self) -> NonNull<sys::jack_client_t> {
        self.client.client
    }
    /// Returns the dynamically loaded JACK library currently used by this client.
    #[inline]
    pub fn library(&self) -> &sys::Jack {
        self.client.lib
    }
}

impl<Context, PortData> Drop for Client<Context, PortData>
where
    Context: MainThreadContext,
    PortData: Send + 'static,
{
    #[inline]
    fn drop(&mut self) {
        unsafe {
            self.client.lib.jack_client_close(self.client.as_ptr());
        }
    }
}

/// An error that occurred when attempting to register a port.
#[derive(Debug, Copy, Clone)]
pub struct PortRegisterError(usize);

impl std::error::Error for PortRegisterError {}

impl std::fmt::Display for PortRegisterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "failed to register port numbered {} in register_ports",
            self.0
        )
    }
}

/// An error that occurred when attempting to activate a client.
pub struct ActivateError<Context, Process, Notify>
where
    Context: MainThreadContext,
    Process: ProcessHandler,
    Notify: NotificationHandler,
{
    /// The client passed when calling [`activate`](InactiveClient::activate).
    pub client: InactiveClient<Context, Process::PortData>,
    /// The process handler passed when calling [`activate`](InactiveClient::activate).
    pub process_handler: Option<Process>,
    /// The notification handler passed when calling [`activate`](InactiveClient::activate).
    pub notification_handler: Option<Notify>,
}

impl<Context, Process, Notify> std::fmt::Debug for ActivateError<Context, Process, Notify>
where
    Context: MainThreadContext,
    Process: ProcessHandler,
    Notify: NotificationHandler,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ActivateError").finish()
    }
}

impl<Context, Process, Notify> std::fmt::Display for ActivateError<Context, Process, Notify>
where
    Context: MainThreadContext,
    Process: ProcessHandler,
    Notify: NotificationHandler,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("failed to activate JACK client")
    }
}

impl<Context, Process, Notify> std::error::Error for ActivateError<Context, Process, Notify>
where
    Context: MainThreadContext,
    Process: ProcessHandler,
    Notify: NotificationHandler,
{
}

/// An opaque JACK client handle for an inactive client.
///
/// Callers should set up their client for operation and then call
/// [`activate`](InactiveClient::activate).
#[doc(alias = "jack_client_t")]
#[derive(Debug)]
#[repr(transparent)]
pub struct InactiveClient<Context, PortData = ()>(Client<Context, PortData>)
where
    Context: MainThreadContext,
    PortData: Send + 'static;

impl<Context, PortData> InactiveClient<Context, PortData>
where
    Context: MainThreadContext,
    PortData: Send + 'static,
{
    /// Tell the JACK server that the program is ready to start processing audio.
    #[doc(alias = "jack_activate")]
    pub fn activate<Process, Notify>(
        self,
        process_handler: Option<Process>,
        notification_handler: Option<Notify>,
        flags: ActivateFlags,
    ) -> Result<ActiveClient<Context, Process, Notify>, ActivateError<Context, Process, Notify>>
    where
        Process: ProcessHandler<PortData = PortData>,
        Notify: NotificationHandler,
    {
        let (process_data, mut res) = if let Some(handler) = process_handler {
            let process_data = Box::new(ProcessData {
                client: self.client.clone(),
                ports: self.process_ports.clone(),
                handler,
            });
            let res = unsafe { crate::set_process_callbacks(&self.client, &*process_data) };
            (Some(UnsafeCell::new(process_data)), res)
        } else {
            let res = unsafe { crate::unset_process_callbacks(&self.client) };
            (None, res)
        };

        let notification_data = if res.is_ok() {
            if notification_handler.is_some() {
                let (tx, rx) = futures_channel::mpsc::unbounded();
                let notification_data = Box::new(NotificationData { tx });
                res =
                    unsafe { crate::set_notification_callbacks(&self.client, &*notification_data) };
                Some((UnsafeCell::new(notification_data), rx))
            } else {
                res = unsafe { crate::unset_notification_callbacks(&self.client) };
                None
            }
        } else {
            None
        };

        if let Some((notification_data, _)) = notification_data.as_ref() {
            res = res.and_then(|_| {
                if flags.contains(ActivateFlags::LATENCY) {
                    unsafe { crate::set_latency_callback(&self.client, &**notification_data.get()) }
                } else {
                    unsafe { crate::unset_latency_callback(&self.client) }
                }
            })
        }
        if let Some(process_data) = process_data.as_ref() {
            res = res.and_then(|_| {
                if flags.contains(ActivateFlags::SYNC) {
                    unsafe { crate::set_sync_callback(&self.client, &**process_data.get()) }
                } else {
                    unsafe { crate::unset_sync_callback(&self.client) }
                }
            });
            res = res.and_then(|_| {
                if flags.contains(ActivateFlags::TIMEBASE) {
                    unsafe {
                        crate::set_timebase_callback(
                            &self.client,
                            !flags.contains(ActivateFlags::TIMEBASE_FORCE),
                            &**process_data.get(),
                        )
                    }
                } else {
                    Ok(())
                }
            });
        }
        let res = res.map(|_| {
            Error::check_ret(unsafe { self.client.lib.jack_activate(self.client.as_ptr()) })
        });

        if res.is_err() {
            unsafe {
                if flags.contains(ActivateFlags::TIMEBASE) {
                    self.client.lib.jack_release_timebase(self.client.as_ptr());
                }
                self.client.lib.jack_deactivate(self.client.as_ptr());
            }
            return Err(ActivateError {
                client: self,
                process_handler: process_data.map(|p| p.into_inner().handler),
                notification_handler,
            });
        }

        let notification_data = if let Some((notification_data, mut rx)) = notification_data {
            let (end_tx, end_rx) = futures_channel::oneshot::channel();
            let guard = ThreadGuard::new();
            let mut handler = notification_handler.unwrap();
            self.context.spawn_local(async move {
                use futures_util::StreamExt;
                guard
                    .check("Expected spawn_local to spawn future on the same thread as the Client");
                while let Some(msg) = rx.next().await {
                    handler.notification(msg);
                }
                end_tx.send(handler).ok();
            });
            Some((notification_data, end_rx))
        } else {
            None
        };
        Ok(ActiveClient {
            client: self.0,
            process_data,
            notification_data,
        })
    }
}

impl<Context, PortData> std::ops::Deref for InactiveClient<Context, PortData>
where
    Context: MainThreadContext,
    PortData: Send + 'static,
{
    type Target = Client<Context, PortData>;
    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<Context, PortData> AsRef<Client<Context, PortData>> for InactiveClient<Context, PortData>
where
    Context: MainThreadContext,
    PortData: Send + 'static,
{
    #[inline]
    fn as_ref(&self) -> &Client<Context, PortData> {
        &self.0
    }
}

/// An error that occurred when attempting to deactivate a client.
///
/// Contains the active client.
pub struct DeactivateError<Context, Process, Notify>
where
    Context: MainThreadContext,
    Process: ProcessHandler,
    Notify: NotificationHandler,
{
    /// The client passed when calling [`deactivate`](ActiveClient::deactivate).
    pub client: ActiveClient<Context, Process, Notify>,
}

impl<Context, Process, Notify> std::fmt::Debug for DeactivateError<Context, Process, Notify>
where
    Context: MainThreadContext,
    Process: ProcessHandler,
    Notify: NotificationHandler,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeactivateError").finish()
    }
}

impl<Context, Process, Notify> std::fmt::Display for DeactivateError<Context, Process, Notify>
where
    Context: MainThreadContext,
    Process: ProcessHandler,
    Notify: NotificationHandler,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("failed to activate JACK client")
    }
}

impl<Context, Process, Notify> std::error::Error for DeactivateError<Context, Process, Notify>
where
    Context: MainThreadContext,
    Process: ProcessHandler,
    Notify: NotificationHandler,
{
}

/// An opaque JACK client handle for an active client.
///
/// Contains methods that can only be used by active clients. Call [`Self::deactivate`] to get an
/// [`InactiveClient`] back.
#[doc(alias = "jack_client_t")]
#[derive(Debug)]
pub struct ActiveClient<Context, Process, Notify>
where
    Context: MainThreadContext,
    Process: ProcessHandler,
    Notify: NotificationHandler,
{
    client: Client<Context, Process::PortData>,
    process_data: Option<UnsafeCell<Box<ProcessData<Process>>>>,
    notification_data: Option<(
        UnsafeCell<Box<NotificationData>>,
        futures_channel::oneshot::Receiver<Notify>,
    )>,
}

impl<Context, Process, Notify> ActiveClient<Context, Process, Notify>
where
    Context: MainThreadContext,
    Process: ProcessHandler,
    Notify: NotificationHandler,
{
    #[inline]
    fn _deactivate(&self) -> crate::Result<()> {
        unsafe {
            self.client
                .client
                .lib
                .jack_release_timebase(self.client.client.as_ptr());
            Error::check_ret(
                self.client
                    .client
                    .lib
                    .jack_deactivate(self.client.client.as_ptr()),
            )?;
        }
        Ok(())
    }
    /// Tell the JACK server to remove this client from the process graph.
    ///
    /// Also, disconnect all ports belonging to it, since inactive clients have no port
    /// connections.
    #[doc(alias = "jack_deactivate")]
    pub async fn deactivate(
        self,
    ) -> Result<
        (
            Client<Context, Process::PortData>,
            Option<Process>,
            Option<Notify>,
        ),
        DeactivateError<Context, Process, Notify>,
    > {
        if self._deactivate().is_err() {
            return Err(DeactivateError { client: self });
        }
        let process = self.process_data.map(|p| p.into_inner().handler);
        let notify = if let Some((data, rx)) = self.notification_data {
            drop(data); // must drop this first to close the notification channel
            Some(rx.await.unwrap())
        } else {
            None
        };
        Ok((self.client, process, notify))
    }
    /// Register as a slow-sync client.
    ///
    /// If [`ActivateFlags::SYNC`] was not used when activating the client, then this method must
    /// be called to start receiving [`ProcessHandler::sync`].
    ///
    /// Slow-sync clients cannot respond immediately to transport position changes.
    /// [`ProcessHandler::sync`] will be invoked at the first available opportunity after its
    /// registration is complete. If the client is currently active this will be the following
    /// process cycle, otherwise it will be the first cycle after calling
    /// [`InactiveClient::activate`]. After that, it runs according to the rules specified on
    /// [`ProcessHandler::sync`]. Clients that don't set a sync callback are assumed to be ready
    /// immediately any time the transport wants to start.
    #[doc(alias = "jack_set_sync_callback")]
    pub fn register_sync_callback(&self) -> crate::Result<()> {
        self.process_data
            .as_ref()
            .ok_or_else(Error::invalid_option)
            .and_then(|process_data| unsafe {
                crate::set_sync_callback(&self.client.client, &**process_data.get())
            })
    }
    /// Unregister as a slow-sync client.
    ///
    /// Undoes the effects of [`Self::register_sync_callback`]. After calling this,
    /// [`ProcessHandler::sync`] will not be received anymore. See the description of
    /// those methods for more information about sync callbacks.
    #[doc(alias = "jack_set_sync_callback")]
    pub fn unregister_sync_callback(&self) -> crate::Result<()> {
        unsafe { crate::unset_sync_callback(&self.client.client) }
    }
    /// Register as timebase master for the JACK subsystem.
    ///
    /// If [`ActivateFlags::TIMEBASE`] was not used when activating the client, then this method
    /// must be called to start receiving [`ProcessHandler::timebase`].
    ///
    /// The timebase master registers a callback that updates extended position information such as
    /// beats or timecode whenever necessary. Without this extended information, there is no need
    /// for this method.
    ///
    /// There is never more than one master at a time. When a new client takes over, the former
    /// [`ProcessHandler::timebase`] is no longer called. Taking over the timebase may be done
    /// conditionally; if `force` is `false`, then this method returns `Err` if there was a
    /// master already.
    #[doc(alias = "jack_set_timebase_callback")]
    pub fn acquire_timebase(&self, force: bool) -> crate::Result<()> {
        self.process_data
            .as_ref()
            .ok_or_else(Error::invalid_option)
            .and_then(|process_data| unsafe {
                crate::set_timebase_callback(&self.client.client, !force, &**process_data.get())
            })
    }
    /// Called by the timebase master to release itself from that responsibility.
    ///
    /// Undoes the effects of [`Self::acquire_timebase`]. After calling this,
    /// [`ProcessHandler::timebase`] will not be received anymore. See the description of those
    /// methods for more information about timebase callbacks.
    ///
    /// If the timebase master releases the timebase or leaves the JACK graph for any reason, the
    /// JACK engine takes over at the start of the next process cycle.  The transport state does
    /// not change. If rolling, it continues to play, with frame numbers as the only available
    /// position information.
    #[doc(alias = "jack_release_timebase")]
    pub fn release_timebase(&self) -> crate::Result<()> {
        Error::check_ret(unsafe {
            self.client
                .client
                .lib
                .jack_release_timebase(self.client.client.as_ptr())
        })
    }
}

impl<Context, Process, Notify> std::ops::Deref for ActiveClient<Context, Process, Notify>
where
    Context: MainThreadContext,
    Process: ProcessHandler,
    Notify: NotificationHandler,
{
    type Target = Client<Context, Process::PortData>;
    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.client
    }
}

impl<Context, Process, Notify> AsRef<Client<Context, Process::PortData>>
    for ActiveClient<Context, Process, Notify>
where
    Context: MainThreadContext,
    Process: ProcessHandler,
    Notify: NotificationHandler,
{
    #[inline]
    fn as_ref(&self) -> &Client<Context, Process::PortData> {
        &self.client
    }
}

/// Builder structure for registering ports.
///
/// Used with [`Client::register_ports`].
#[derive(Clone, Debug)]
pub struct PortInfo<'s, PortData: Send + 'static> {
    /// The short name of the port.
    ///
    /// Each port has a short name. The port's full name contains the name of the client
    /// concatenated with a colon (`:`) followed by its short name.  The [`crate::port_name_size`]
    /// is the maximum length of this full name. Exceeding that will cause the port registration to
    /// fail and return `None`.
    ///
    /// The port name must be unique among all ports owned by this client. If the name is not
    /// unique, the registration will fail and return `None`.
    pub name: &'s str,
    /// The type of data for this port.
    ///
    /// All ports have a type, which currently may be audio sample data or MIDI data.
    pub type_: PortType,
    /// Whether this port is an input or output port.
    pub mode: PortMode,
    /// Additional flags for creating this port.
    pub flags: PortCreateFlags,
    /// User-supplied data associated with this port.
    ///
    /// Since the process thread cannot allocate data on the heap, this field should be used to
    /// store any data required by this port to run its processing. This data must be [`Send`] so
    /// it can be sent into the process thread, where it can be accessed by calling
    /// [`ProcessPort::data`](crate::ProcessPort::data).
    pub data: PortData,
}

impl<'s, PortData: Send + 'static> PortInfo<'s, PortData> {
    /// Convenience constructor for creating audio input ports.
    #[inline]
    pub fn audio_in(name: &'s str, data: PortData) -> Self {
        Self {
            name,
            type_: PortType::Audio,
            mode: PortMode::Input,
            flags: PortCreateFlags::empty(),
            data,
        }
    }
    /// Convenience constructor for creating audio output ports.
    #[inline]
    pub fn audio_out(name: &'s str, data: PortData) -> Self {
        Self {
            name,
            type_: PortType::Audio,
            mode: PortMode::Output,
            flags: PortCreateFlags::empty(),
            data,
        }
    }
    /// Convenience constructor for creating MIDI input ports.
    #[inline]
    pub fn midi_in(name: &'s str, data: PortData) -> Self {
        Self {
            name,
            type_: PortType::Midi,
            mode: PortMode::Input,
            flags: PortCreateFlags::empty(),
            data,
        }
    }
    /// Convenience constructor for creating MIDI output ports.
    #[inline]
    pub fn midi_out(name: &'s str, data: PortData) -> Self {
        Self {
            name,
            type_: PortType::Midi,
            mode: PortMode::Output,
            flags: PortCreateFlags::empty(),
            data,
        }
    }
    /// Set the flags for creating this port.
    ///
    /// This method is meant to be used with the builder pattern:
    ///
    /// ```ignore
    /// client.register_ports([
    ///     PortInfo::audio_in("in L", ()).flags(PortCreateFlags::CAN_MONITOR),
    ///     PortInfo::audio_in("in R", ()).flags(PortCreateFlags::CAN_MONITOR),
    /// ]);
    /// ```
    #[inline]
    pub fn flags(mut self, flags: PortCreateFlags) -> Self {
        self.flags = flags;
        self
    }
}

/// A key uniquely identifying a port owned by a local [`Client`](crate::Client).
///
/// Represented internally by a 64-bit UUID. This value should be used as to reference a client's
/// own ports.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct OwnedPortUuid(Uuid);

impl OwnedPortUuid {
    #[inline]
    pub(crate) unsafe fn new(client: &ClientHandle, ptr: *mut sys::jack_port_t) -> Option<Self> {
        NonNull::new(ptr)?;
        let uuid = client.lib.jack_port_uuid(ptr);
        Some(Self(Uuid(NonZeroU64::new(uuid)?)))
    }
    /// Returns the unique ID of this port.
    #[inline]
    pub fn uuid(&self) -> Uuid {
        self.0
    }
}

impl std::fmt::Display for OwnedPortUuid {
    #[inline]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

pub(crate) struct ProcessData<P: ProcessHandler> {
    pub(crate) client: ClientHandle,
    pub(crate) ports: Arc<AtomicTripleBuffer<ProcessPorts<P::PortData>>>,
    pub(crate) handler: P,
}

pub(crate) struct NotificationData {
    pub(crate) tx: futures_channel::mpsc::UnboundedSender<Notification>,
}
