use crate::{
    sys, ClientHandle, Error, NotificationData, PortId, ProcessData, ProcessScope,
    TransportPosition, TransportState, Uuid,
};
use std::{
    ffi::{CStr, CString},
    marker::PhantomData,
    num::{NonZeroU32, NonZeroU64},
    ops::ControlFlow,
    os::raw::{c_char, c_int, c_void},
};

/// Handler for the process thread.
///
/// Implementations of this trait are passed to
/// [`InactiveClient::activate`](crate::InactiveClient::activate). The object will be
/// passed into a separate real-time process thread, and will have all of its methods called there.
/// Because of this, implementations must be [`Send`].
///
/// Due to the [`Send`] requirement, and the additional restrictions on allocations and blocking
/// inside [`process`](Self::process), there are heavy limits on what can be contained in structs
/// implementing this trait (and in the associated `PortData` type). The following types are the
/// only ones recommended for concurrent use in real-time threads:
///
/// - [`Arc`](std::sync::Arc) containing an integer type from [`std::sync::atomic`].
/// - A bounded lock-free channel or ring buffer such as
///   [`crossbeam::channel::bounded`](https://docs.rs/crossbeam/0.8/crossbeam/channel/fn.bounded.html),
///   [`flume::bounded`](https://docs.rs/flume/0.10/flume/fn.bounded.html),
///   [`ringbuf`](https://docs.rs/ringbuf), or [`rtrb`](https://docs.rs/rtrb/). The messages should
///   not contain any heap allocated data, unless it can be guaranteed the data will not be
///   allocated or deallocated in the process thread.
/// - <code>[Arc](std::sync::Arc)&lt;[AtomicTripleBuffer](atb::AtomicTripleBuffer)></code> is
///   recommended if any complex heap data needs to be periodically allocated/reallocated and
///   exchanged with the process thread.
/// - Smart pointer types that can be garbage collected on a different thread, such as in
///   [`basedrop`](https://docs.rs/basedrop/).
///
/// Do not use unbounded channels, mutexes, or locks of any kind to exchange data with the process
/// thread.
#[allow(unused)]
pub trait ProcessHandler: Send + 'static {
    /// Associated data for ports.
    ///
    /// Any per-port data that [`Self::process`] needs to access should be contained in this type.
    /// Because the process thread cannot allocate or deallocate any data, any storage that the
    /// port needs should be fully allocated to its max capacity when constructing this type. The
    /// port data can be accessed with [`ProcessPort::data`](crate::ProcessPort::data).
    ///
    /// Objects of this type will be sent into the process thread and will only be readable and
    /// writable from there. To communicate with the port from other threads, the data should
    /// contain communication primitives that don't block or dynamically allocate, such as a
    /// bounded channel receiver or an [`Arc`](std::sync::Arc) containing an atomic lock-free type
    /// such as [`AtomicTripleBuffer`](atb::AtomicTripleBuffer). See the documentation on
    /// [`ProcessHandler`] for more information.
    ///
    /// Clients that support multiple port types will want to make this an enum.
    type PortData: Send + 'static;
    #[doc(alias = "JackThreadInitCallback")]
    #[doc(alias = "jack_set_thread_init_callback")]
    #[inline]
    fn thread_init(&mut self) {}
    #[doc(alias = "JackProcessCallback")]
    #[doc(alias = "jack_set_process_callback")]
    #[inline]
    fn process(&mut self, scope: ProcessScope<Self::PortData>) -> ControlFlow<(), ()> {
        ControlFlow::Continue(())
    }
    #[doc(alias = "JackSyncCallback")]
    #[doc(alias = "jack_set_sync_callback")]
    #[inline]
    fn sync(
        &mut self,
        state: TransportState,
        pos: &TransportPosition,
        scope: ProcessScope<Self::PortData>,
    ) -> ControlFlow<(), ()> {
        ControlFlow::Continue(())
    }
    #[doc(alias = "JackTimebaseCallback")]
    #[doc(alias = "jack_set_timebase_callback")]
    #[inline]
    fn timebase(
        &mut self,
        state: TransportState,
        pos: &mut TransportPosition,
        new_pos: bool,
        scope: ProcessScope<Self::PortData>,
    ) {
    }
}

impl ProcessHandler for () {
    type PortData = ();
}

/// A simple [`ProcessHandler`] for running the process callback.
///
/// Intended for simple clients. All other process thread events are ignored.
pub struct ClosureProcessHandler<Func, PortData = ()>(Func, PhantomData<PortData>)
where
    Func: for<'a> FnMut(ProcessScope<'a, PortData>) -> ControlFlow<(), ()> + Send + 'static,
    PortData: Send + 'static;

impl<Func, PortData> ClosureProcessHandler<Func, PortData>
where
    Func: for<'a> FnMut(ProcessScope<'a, PortData>) -> ControlFlow<(), ()> + Send + 'static,
    PortData: Send + 'static,
{
    /// Creates a new process handler running `func` as the process callback.
    #[inline]
    pub fn new(func: Func) -> Self {
        Self(func, PhantomData)
    }
}

impl<Func, PortData> ProcessHandler for ClosureProcessHandler<Func, PortData>
where
    Func: for<'a> FnMut(ProcessScope<'a, PortData>) -> ControlFlow<(), ()> + Send + 'static,
    PortData: Send + 'static,
{
    type PortData = PortData;
    #[inline]
    fn process(&mut self, scope: ProcessScope<Self::PortData>) -> ControlFlow<(), ()> {
        self.0(scope)
    }
}

/// Data type for client notification messages.
///
/// All enum variants correspond to a method on [`NotificationHandler`]. These messages will all
/// be received on the thread the client was created on.
#[derive(Debug, Clone)]
pub enum Notification {
    #[doc(alias = "JackShutdownCallback")]
    #[doc(alias = "jack_on_shutdown")]
    Shutdown,
    #[doc(alias = "JackFreewheelCallback")]
    #[doc(alias = "jack_set_freewheel_callback")]
    Freewheel { enabled: bool },
    #[doc(alias = "JackBufferSizeCallback")]
    #[doc(alias = "jack_set_buffer_size_callback")]
    BufferSize { frames: u32 },
    #[doc(alias = "JackSampleRateCallback")]
    #[doc(alias = "jack_set_sample_rate_callback")]
    SampleRate { frames: u32 },
    #[doc(alias = "JackClientRegistrationCallback")]
    #[doc(alias = "jack_set_client_registration_callback")]
    ClientRegister { name: CString },
    #[doc(alias = "JackClientRegistrationCallback")]
    #[doc(alias = "jack_set_client_registration_callback")]
    ClientUnregister { name: CString },
    #[doc(alias = "JackPortRegistrationCallback")]
    #[doc(alias = "jack_set_port_registration_callback")]
    PortRegister { port: PortId },
    #[doc(alias = "JackPortRegistrationCallback")]
    #[doc(alias = "jack_set_port_registration_callback")]
    PortUnregister { port: PortId },
    #[doc(alias = "JackPortConnectCallback")]
    #[doc(alias = "jack_set_port_connect_callback")]
    PortConnect { a: PortId, b: PortId },
    #[doc(alias = "JackPortConnectCallback")]
    #[doc(alias = "jack_set_port_connect_callback")]
    PortDisconnect { a: PortId, b: PortId },
    #[doc(alias = "JackPortRenameCallback")]
    #[doc(alias = "jack_set_port_rename_callback")]
    PortRename {
        port: PortId,
        old_name: CString,
        new_name: CString,
    },
    #[doc(alias = "JackGraphOrderCallback")]
    #[doc(alias = "jack_set_graph_order_callback")]
    GraphOrder,
    #[doc(alias = "JackXRunCallback")]
    #[doc(alias = "jack_set_xrun_callback")]
    XRun,
    #[doc(alias = "JackLatencyCallback")]
    #[doc(alias = "jack_set_latency_callback")]
    Latency { mode: LatencyMode },
    #[doc(alias = "JackPropertyChangeCallback")]
    #[doc(alias = "jack_set_property_change_callback")]
    PropertyChange {
        subject: Uuid,
        key: Option<CString>,
        change: PropertyChange,
    },
}

/// Handler for notifications on the client thread.
///
/// Implementations of this trait are passed to
/// [`InactiveClient::activate`](crate::InactiveClient::activate). Notifications are optional,
/// informational messages sent by the JACK server when an event occurs. All methods on this trait
/// will be called on the main client thread.
#[allow(unused)]
pub trait NotificationHandler: 'static {
    /// Dispatch a single notification.
    ///
    /// This method is the only method called directly by the client. The default implementation
    /// forwards the message to the rest of the methods on this trait. If this method is
    /// implemented then no other methods on this trait need to be implemented.
    ///
    /// Users should not normally need to implement this, unless the notification needs to be sent
    /// to another thread. Simple implementations can use [`ClosureNotificationHandler`] instead of
    /// manually implementing this method.
    #[inline]
    fn notification(&mut self, msg: Notification) {
        use Notification::*;
        match msg {
            Shutdown => self.shutdown(),
            Freewheel { enabled } => self.freewheel(enabled),
            BufferSize { frames } => self.buffer_size(frames),
            SampleRate { frames } => self.sample_rate(frames),
            ClientRegister { name } => self.client_register(name),
            ClientUnregister { name } => self.client_unregister(name),
            PortRegister { port } => self.port_register(port),
            PortUnregister { port } => self.port_unregister(port),
            PortConnect { a, b } => self.port_connect(a, b),
            PortDisconnect { a, b } => self.port_disconnect(a, b),
            PortRename {
                port,
                old_name,
                new_name,
            } => self.port_rename(port, old_name, new_name),
            GraphOrder => self.graph_order(),
            XRun => self.xrun(),
            Latency { mode } => self.latency(mode),
            PropertyChange {
                subject,
                key,
                change,
            } => self.property_change(subject, key, change),
        }
    }
    #[doc(alias = "JackShutdownCallback")]
    #[doc(alias = "jack_on_shutdown")]
    #[inline]
    fn shutdown(&mut self) {}
    #[doc(alias = "JackFreewheelCallback")]
    #[doc(alias = "jack_set_freewheel_callback")]
    #[inline]
    fn freewheel(&mut self, enabled: bool) {}
    #[doc(alias = "JackBufferSizeCallback")]
    #[doc(alias = "jack_set_buffer_size_callback")]
    #[inline]
    fn buffer_size(&mut self, frames: u32) {}
    #[doc(alias = "JackSampleRateCallback")]
    #[doc(alias = "jack_set_sample_rate_callback")]
    #[inline]
    fn sample_rate(&mut self, frames: u32) {}
    #[doc(alias = "JackClientRegistrationCallback")]
    #[doc(alias = "jack_set_client_registration_callback")]
    #[inline]
    fn client_register(&mut self, name: CString) {}
    #[doc(alias = "JackClientRegistrationCallback")]
    #[doc(alias = "jack_set_client_registration_callback")]
    #[inline]
    fn client_unregister(&mut self, name: CString) {}
    #[doc(alias = "JackPortRegistrationCallback")]
    #[doc(alias = "jack_set_port_registration_callback")]
    #[inline]
    fn port_register(&mut self, port: PortId) {}
    #[doc(alias = "JackPortRegistrationCallback")]
    #[doc(alias = "jack_set_port_registration_callback")]
    #[inline]
    fn port_unregister(&mut self, port: PortId) {}
    #[doc(alias = "JackPortConnectCallback")]
    #[doc(alias = "jack_set_port_connect_callback")]
    #[inline]
    fn port_connect(&mut self, a: PortId, b: PortId) {}
    #[doc(alias = "JackPortConnectCallback")]
    #[doc(alias = "jack_set_port_connect_callback")]
    #[inline]
    fn port_disconnect(&mut self, a: PortId, b: PortId) {}
    #[doc(alias = "JackPortRenameCallback")]
    #[doc(alias = "jack_set_port_rename_callback")]
    #[inline]
    fn port_rename(&mut self, port: PortId, old_name: CString, new_name: CString) {}
    #[doc(alias = "JackGraphOrderCallback")]
    #[doc(alias = "jack_set_graph_order_callback")]
    #[inline]
    fn graph_order(&mut self) {}
    #[doc(alias = "JackXRunCallback")]
    #[doc(alias = "jack_set_xrun_callback")]
    #[inline]
    fn xrun(&mut self) {}
    #[doc(alias = "JackLatencyCallback")]
    #[doc(alias = "jack_set_latency_callback")]
    #[inline]
    fn latency(&mut self, mode: LatencyMode) {}
    #[doc(alias = "JackPropertyChangeCallback")]
    #[doc(alias = "jack_set_property_change_callback")]
    #[inline]
    fn property_change(&mut self, subject: Uuid, key: Option<CString>, change: PropertyChange) {}
}

impl NotificationHandler for () {}

/// A simple implementation of [`NotificationHandler`] that passes all notifications to a function
/// closure.
pub struct ClosureNotificationHandler<F: FnMut(Notification) + 'static>(F);

impl<F: FnMut(Notification) + 'static> ClosureNotificationHandler<F> {
    /// Create a new notification handler.
    ///
    /// The handler simply forwards all notifications to `func`.
    #[inline]
    pub fn new(func: F) -> Self {
        Self(func)
    }
}

impl<F: FnMut(Notification) + 'static> NotificationHandler for ClosureNotificationHandler<F> {
    #[inline]
    fn notification(&mut self, msg: Notification) {
        self.0(msg);
    }
}

#[doc(alias = "JackLatencyCallbackMode")]
#[doc(alias = "jack_latency_callback_mode_t")]
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub enum LatencyMode {
    #[doc(alias = "JackCaptureLatency")]
    Capture,
    #[doc(alias = "JackPlaybackLatency")]
    Playback,
}

impl LatencyMode {
    #[inline]
    pub(crate) fn from_jack(mode: sys::jack_latency_callback_mode_t) -> Option<Self> {
        match mode {
            sys::JackLatencyCallbackMode_JackCaptureLatency => Some(Self::Capture),
            sys::JackLatencyCallbackMode_JackPlaybackLatency => Some(Self::Playback),
            _ => None,
        }
    }
    #[inline]
    pub(crate) fn into_jack(self) -> sys::jack_latency_callback_mode_t {
        match self {
            Self::Capture => sys::JackLatencyCallbackMode_JackCaptureLatency,
            Self::Playback => sys::JackLatencyCallbackMode_JackPlaybackLatency,
        }
    }
}

#[doc(alias = "jack_property_change_t")]
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub enum PropertyChange {
    #[doc(alias = "PropertyCreated")]
    Created,
    #[doc(alias = "PropertyChanged")]
    Changed,
    #[doc(alias = "PropertyDeleted")]
    Deleted,
}

impl PropertyChange {
    #[inline]
    pub(crate) fn from_jack(change: sys::jack_property_change_t) -> Self {
        match change {
            sys::jack_property_change_t_PropertyCreated => Self::Created,
            sys::jack_property_change_t_PropertyChanged => Self::Changed,
            sys::jack_property_change_t_PropertyDeleted => Self::Deleted,
            _ => Self::Changed,
        }
    }
}

unsafe extern "C" fn thread_init_callback<P: ProcessHandler>(arg: *mut c_void) {
    let data = &mut *(arg as *mut ProcessData<P>);
    data.handler.thread_init();
}

unsafe extern "C" fn process_callback<P: ProcessHandler>(
    frames: sys::jack_nframes_t,
    arg: *mut c_void,
) -> c_int {
    let data = &mut *(arg as *mut ProcessData<P>);
    let mut front = data.ports.front_buffer().unwrap();
    let scope = ProcessScope {
        client: &data.client,
        ports: &mut *front,
        nframes: frames,
    };
    match data.handler.process(scope) {
        ControlFlow::Continue(_) => 0,
        ControlFlow::Break(_) => -1,
    }
}

#[inline]
pub(crate) unsafe fn set_process_callbacks<P: ProcessHandler>(
    client: &ClientHandle,
    process: *const ProcessData<P>,
) -> crate::Result<()> {
    Error::check_ret(client.lib.jack_set_thread_init_callback(
        client.as_ptr(),
        Some(thread_init_callback::<P>),
        process as *mut _,
    ))?;
    Error::check_ret(client.lib.jack_set_process_callback(
        client.as_ptr(),
        Some(process_callback::<P>),
        process as *mut _,
    ))?;
    Ok(())
}

#[inline]
pub(crate) unsafe fn unset_process_callbacks(client: &ClientHandle) -> crate::Result<()> {
    Error::check_ret(client.lib.jack_set_thread_init_callback(
        client.as_ptr(),
        None,
        std::ptr::null_mut(),
    ))?;
    Error::check_ret(client.lib.jack_set_process_callback(
        client.as_ptr(),
        None,
        std::ptr::null_mut(),
    ))?;
    Ok(())
}

unsafe extern "C" fn shutdown_callback(arg: *mut c_void) {
    let data = &*(arg as *const NotificationData);
    data.tx.unbounded_send(Notification::Shutdown).ok();
}

unsafe extern "C" fn freewheel_callback(starting: c_int, arg: *mut c_void) {
    let data = &*(arg as *const NotificationData);
    let enabled = starting != 0;
    data.tx
        .unbounded_send(Notification::Freewheel { enabled })
        .ok();
}

unsafe extern "C" fn buffer_size_callback(frames: sys::jack_nframes_t, arg: *mut c_void) -> c_int {
    let data = &*(arg as *const NotificationData);
    data.tx
        .unbounded_send(Notification::BufferSize { frames })
        .ok();
    0
}

unsafe extern "C" fn sample_rate_callback(frames: sys::jack_nframes_t, arg: *mut c_void) -> c_int {
    let data = &*(arg as *const NotificationData);
    data.tx
        .unbounded_send(Notification::SampleRate { frames })
        .ok();
    0
}

unsafe extern "C" fn client_registration_callback(
    name: *const c_char,
    register: c_int,
    arg: *mut c_void,
) {
    let data = &*(arg as *const NotificationData);
    let name = CStr::from_ptr(name).into();
    if register != 0 {
        data.tx
            .unbounded_send(Notification::ClientRegister { name })
            .ok();
    } else {
        data.tx
            .unbounded_send(Notification::ClientUnregister { name })
            .ok();
    }
}

unsafe extern "C" fn port_registration_callback(
    port: sys::jack_port_id_t,
    register: c_int,
    arg: *mut c_void,
) {
    let data = &*(arg as *const NotificationData);
    let port = PortId(NonZeroU32::new_unchecked(port));
    if register != 0 {
        data.tx
            .unbounded_send(Notification::PortRegister { port })
            .ok();
    } else {
        data.tx
            .unbounded_send(Notification::PortUnregister { port })
            .ok();
    }
}

unsafe extern "C" fn port_connect_callback(
    a: sys::jack_port_id_t,
    b: sys::jack_port_id_t,
    connect: c_int,
    arg: *mut c_void,
) {
    let data = &*(arg as *const NotificationData);
    let a = PortId(NonZeroU32::new_unchecked(a));
    let b = PortId(NonZeroU32::new_unchecked(b));
    if connect != 0 {
        data.tx
            .unbounded_send(Notification::PortConnect { a, b })
            .ok();
    } else {
        data.tx
            .unbounded_send(Notification::PortDisconnect { a, b })
            .ok();
    }
}

unsafe extern "C" fn port_rename_callback(
    port: sys::jack_port_id_t,
    old_name: *const c_char,
    new_name: *const c_char,
    arg: *mut c_void,
) {
    let data = &*(arg as *const NotificationData);
    let port = PortId(NonZeroU32::new_unchecked(port));
    let old_name = CStr::from_ptr(old_name).into();
    let new_name = CStr::from_ptr(new_name).into();
    data.tx
        .unbounded_send(Notification::PortRename {
            port,
            old_name,
            new_name,
        })
        .ok();
}

unsafe extern "C" fn graph_order_callback(arg: *mut c_void) -> c_int {
    let data = &*(arg as *const NotificationData);
    data.tx.unbounded_send(Notification::GraphOrder).ok();
    0
}

unsafe extern "C" fn xrun_callback(arg: *mut c_void) -> c_int {
    let data = &*(arg as *const NotificationData);
    data.tx.unbounded_send(Notification::XRun).ok();
    0
}

unsafe extern "C" fn property_change_callback(
    subject: sys::jack_uuid_t,
    key: *const c_char,
    change: sys::jack_property_change_t,
    arg: *mut c_void,
) {
    let data = &*(arg as *const NotificationData);
    data.tx
        .unbounded_send(Notification::PropertyChange {
            subject: Uuid(NonZeroU64::new_unchecked(subject)),
            key: if key.is_null() {
                None
            } else {
                Some(CStr::from_ptr(key).into())
            },
            change: PropertyChange::from_jack(change),
        })
        .ok();
}

#[inline]
pub(crate) unsafe fn set_notification_callbacks(
    client: &ClientHandle,
    notification: *const NotificationData,
) -> crate::Result<()> {
    client.lib.jack_on_shutdown(
        client.as_ptr(),
        Some(shutdown_callback),
        notification as *mut _,
    );
    Error::check_ret(client.lib.jack_set_freewheel_callback(
        client.as_ptr(),
        Some(freewheel_callback),
        notification as *mut _,
    ))?;
    Error::check_ret(client.lib.jack_set_buffer_size_callback(
        client.as_ptr(),
        Some(buffer_size_callback),
        notification as *mut _,
    ))?;
    Error::check_ret(client.lib.jack_set_sample_rate_callback(
        client.as_ptr(),
        Some(sample_rate_callback),
        notification as *mut _,
    ))?;
    Error::check_ret(client.lib.jack_set_client_registration_callback(
        client.as_ptr(),
        Some(client_registration_callback),
        notification as *mut _,
    ))?;
    Error::check_ret(client.lib.jack_set_port_registration_callback(
        client.as_ptr(),
        Some(port_registration_callback),
        notification as *mut _,
    ))?;
    Error::check_ret(client.lib.jack_set_port_connect_callback(
        client.as_ptr(),
        Some(port_connect_callback),
        notification as *mut _,
    ))?;
    Error::check_ret(client.lib.jack_set_port_rename_callback(
        client.as_ptr(),
        Some(port_rename_callback),
        notification as *mut _,
    ))?;
    Error::check_ret(client.lib.jack_set_graph_order_callback(
        client.as_ptr(),
        Some(graph_order_callback),
        notification as *mut _,
    ))?;
    Error::check_ret(client.lib.jack_set_xrun_callback(
        client.as_ptr(),
        Some(xrun_callback),
        notification as *mut _,
    ))?;
    Error::check_ret(client.lib.jack_set_property_change_callback(
        client.as_ptr(),
        Some(property_change_callback),
        notification as *mut _,
    ))?;
    Ok(())
}

#[inline]
pub(crate) unsafe fn unset_notification_callbacks(client: &ClientHandle) -> crate::Result<()> {
    client
        .lib
        .jack_on_shutdown(client.as_ptr(), None, std::ptr::null_mut());
    Error::check_ret(client.lib.jack_set_freewheel_callback(
        client.as_ptr(),
        None,
        std::ptr::null_mut(),
    ))?;
    Error::check_ret(client.lib.jack_set_buffer_size_callback(
        client.as_ptr(),
        None,
        std::ptr::null_mut(),
    ))?;
    Error::check_ret(client.lib.jack_set_sample_rate_callback(
        client.as_ptr(),
        None,
        std::ptr::null_mut(),
    ))?;
    Error::check_ret(client.lib.jack_set_client_registration_callback(
        client.as_ptr(),
        None,
        std::ptr::null_mut(),
    ))?;
    Error::check_ret(client.lib.jack_set_port_registration_callback(
        client.as_ptr(),
        None,
        std::ptr::null_mut(),
    ))?;
    Error::check_ret(client.lib.jack_set_port_connect_callback(
        client.as_ptr(),
        None,
        std::ptr::null_mut(),
    ))?;
    Error::check_ret(client.lib.jack_set_port_rename_callback(
        client.as_ptr(),
        None,
        std::ptr::null_mut(),
    ))?;
    Error::check_ret(client.lib.jack_set_graph_order_callback(
        client.as_ptr(),
        None,
        std::ptr::null_mut(),
    ))?;
    Error::check_ret(client.lib.jack_set_xrun_callback(
        client.as_ptr(),
        None,
        std::ptr::null_mut(),
    ))?;
    Error::check_ret(client.lib.jack_set_property_change_callback(
        client.as_ptr(),
        None,
        std::ptr::null_mut(),
    ))?;
    Ok(())
}

unsafe extern "C" fn latency_callback(mode: sys::jack_latency_callback_mode_t, arg: *mut c_void) {
    let data = &*(arg as *const NotificationData);
    let mode = match LatencyMode::from_jack(mode) {
        Some(mode) => mode,
        None => return,
    };
    data.tx.unbounded_send(Notification::Latency { mode }).ok();
}

#[inline]
pub(crate) unsafe fn set_latency_callback(
    client: &ClientHandle,
    notification: *const NotificationData,
) -> crate::Result<()> {
    Error::check_ret(client.lib.jack_set_latency_callback(
        client.as_ptr(),
        Some(latency_callback),
        notification as *mut _,
    ))
}

#[inline]
pub(crate) unsafe fn unset_latency_callback(client: &ClientHandle) -> crate::Result<()> {
    Error::check_ret(client.lib.jack_set_latency_callback(
        client.as_ptr(),
        None,
        std::ptr::null_mut(),
    ))
}

unsafe extern "C" fn sync_callback<P: ProcessHandler>(
    state: sys::jack_transport_state_t,
    pos: *mut sys::jack_position_t,
    arg: *mut c_void,
) -> c_int {
    let data = &mut *(arg as *mut ProcessData<P>);
    let mut front = data.ports.front_buffer().unwrap();
    let scope = ProcessScope {
        client: &data.client,
        ports: &mut *front,
        nframes: 0,
    };
    let state = TransportState::from_jack(state);
    let pos = &*(pos as *mut TransportPosition);
    match data.handler.sync(state, pos, scope) {
        ControlFlow::Continue(_) => 0,
        ControlFlow::Break(_) => -1,
    }
}

#[inline]
pub(crate) unsafe fn set_sync_callback<P: ProcessHandler>(
    client: &ClientHandle,
    process: *const ProcessData<P>,
) -> crate::Result<()> {
    Error::check_ret(client.lib.jack_set_sync_callback(
        client.as_ptr(),
        Some(sync_callback::<P>),
        process as *mut _,
    ))
}

#[inline]
pub(crate) unsafe fn unset_sync_callback(client: &ClientHandle) -> crate::Result<()> {
    Error::check_ret(
        client
            .lib
            .jack_set_sync_callback(client.as_ptr(), None, std::ptr::null_mut()),
    )
}

unsafe extern "C" fn timebase_callback<P: ProcessHandler>(
    state: sys::jack_transport_state_t,
    frames: sys::jack_nframes_t,
    pos: *mut sys::jack_position_t,
    new_pos: c_int,
    arg: *mut c_void,
) {
    let data = &mut *(arg as *mut ProcessData<P>);
    let mut front = data.ports.front_buffer().unwrap();
    let scope = ProcessScope {
        client: &data.client,
        ports: &mut *front,
        nframes: frames,
    };
    let state = TransportState::from_jack(state);
    let pos = &mut *(pos as *mut TransportPosition);
    data.handler.timebase(state, pos, new_pos != 0, scope);
}

#[inline]
pub(crate) unsafe fn set_timebase_callback<P: ProcessHandler>(
    client: &ClientHandle,
    conditional: bool,
    process: *const ProcessData<P>,
) -> crate::Result<()> {
    Error::check_ret(client.lib.jack_set_timebase_callback(
        client.as_ptr(),
        conditional as _,
        Some(timebase_callback::<P>),
        process as *mut _,
    ))
}
