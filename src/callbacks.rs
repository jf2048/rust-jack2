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
    /// Invoked by the engine when the client is inserted in the graph.
    ///
    /// Called once just after the creation of the thread in which all other [`ProcessHandler`]
    /// events will be handled.
    ///
    /// Implementations do not need to be suitable for real-time execution.
    #[doc(alias = "JackThreadInitCallback")]
    #[doc(alias = "jack_set_thread_init_callback")]
    #[inline]
    fn thread_init(&mut self) {}
    /// Invoked by the engine anytime there is audio work to be done.
    ///
    /// The code in the implementation must be suitable for real-time execution. That means that it
    /// cannot call functions that might block for a long time. This includes calling heap
    /// allocating/freeing functions like [`Box::new`] or [`Rc::new`](std::rc::Rc::new), making
    /// modifications to [`String`], [`Vec`] or other collections, causing a
    /// heap-allocated type to [drop](Drop), calling [`Mutex::lock`](std::sync::Mutex::lock),
    /// calling any disk or network I/O functions, calling [`std::thread::sleep`], etc, etc. See
    /// [JACK Design
    /// Documentation](https://web.archive.org/web/20080221160709/http://jackit.sourceforge.net/docs/design/#SECTION00411000000000000000)
    /// for more information.
    ///
    /// Because of these restrictions, it is recommended to limit operations in this method to only
    /// accessing safe, lock-free data structures that have all their storage allocated up-front,
    /// and only performing I/O to and from the JACK ports owned by this client. See the
    /// documentation on [`ProcessHandler`] for more information.
    ///
    /// Return [`ControlFlow::Break`] to cause JACK to remove this client from the processing
    /// graph.
    #[doc(alias = "JackProcessCallback")]
    #[doc(alias = "jack_set_process_callback")]
    #[inline]
    fn process(&mut self, scope: ProcessScope<Self::PortData>) -> ControlFlow<(), ()> {
        ControlFlow::Continue(())
    }
    /// Invoked just before [`process`](Self::process) when client is registered as slow-sync.
    ///
    /// In order for this method to be invoked, you **must** pass
    /// [`ActivateFlags::SYNC`](crate::ActivateFlags::SYNC) when activating the client, or call
    /// [`ActiveClient::register_sync_callback`](crate::ActiveClient::register_sync_callback) after
    /// creating the client.
    ///
    /// When the client is active, this callback is called in the same thread as [`Self::process`].
    /// in the same thread. This occurs once after registration, then subsequently whenever some
    /// client requests a new position, or the transport enters the
    /// [`Starting`](crate::TransportState::Starting) state.  This real-time method must not wait.
    ///
    /// The transport `state` will be:
    ///
    /// - [`Stopped`](crate::TransportState::Stopped) when a new position is requested.
    /// - [`Starting`](crate::TransportState::Starting) when the transport is waiting to start.
    /// - [`Rolling`](crate::TransportState::Rolling) when the timeout has expired, and the
    ///   position is now a moving target.
    ///
    /// Return [`ControlFlow::Break`] when ready to roll.
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
    /// Invoked to provide extended position information.
    ///
    /// In order for this method to be invoked, you **must** pass
    /// [`ActivateFlags::TIMEBASE`](crate::ActivateFlags::TIMEBASE) or
    /// [`ActivateFlags::TIMEBASE_FORCE`](crate::ActivateFlags::TIMEBASE_FORCE) when activating the
    /// client, or call [`ActiveClient::acquire_timebase`](crate::ActiveClient::acquire_timebase)
    /// after creating the client. The output of this method affects all of the following process
    /// cycle. This real-time method must not wait.
    ///
    /// This method is called immediately after [`Self::process`]  in the same thread whenever the
    /// transport is rolling, or when any client has requested a new position in the previous
    /// cycle. The first cycle after
    /// [`ActiveClient::acquire_timebase`](crate::ActiveClient::acquire_timebase) is also treated
    /// as a new position, or the first cycle after
    /// [`InactiveClient::activate`](crate::InactiveClient::activate) if the client had been
    /// inactive.
    ///
    /// The timebase master may not use its `pos` argument to set the current frame. To change
    /// position, use [`Transport::reposition`](crate::Transport::reposition) or
    /// [`Transport::locate`](crate::Transport::locate).
    ///
    /// If `new_pos` is `false`, the `state` object contains extended position information from the
    /// current cycle. If `true`, it contains whatever was set by the requester. The `timebase`
    /// implementation's task is to update the extended information here.
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
    /// Sent whenever the JACK server is shutdown.
    ///
    /// Clients do not normally need to handle this. It exists only to help more complex clients
    /// understand what is going on.
    ///
    /// Note that after server shutdown, the [`Client`](crate::Client) is *not* deallocated, the
    /// application is responsible to properly exit its event loop and release client resources.
    ///
    /// The corresponding trait method is [`NotificationHandler::shutdown`].
    #[doc(alias = "JackShutdownCallback")]
    #[doc(alias = "jack_on_shutdown")]
    Shutdown,
    /// Sent whenever the JACK server starts or stops freewheeling.
    ///
    /// The corresponding trait method is [`NotificationHandler::freewheel`].
    #[doc(alias = "JackFreewheelCallback")]
    #[doc(alias = "jack_set_freewheel_callback")]
    Freewheel {
        /// `true` if starting to freewheel, false otherwise.
        enabled: bool,
    },
    /// Sent when the size of the buffers that will be passed to [`ProcessHandler::process`] is
    /// about to change.
    ///
    /// The corresponding trait method is [`NotificationHandler::buffer_size`].
    #[doc(alias = "JackBufferSizeCallback")]
    #[doc(alias = "jack_set_buffer_size_callback")]
    BufferSize {
        /// New buffer size, in samples.
        frames: u32,
    },
    /// Sent when the engine sample rate changes.
    ///
    /// The corresponding trait method is [`NotificationHandler::sample_rate`].
    #[doc(alias = "JackSampleRateCallback")]
    #[doc(alias = "jack_set_sample_rate_callback")]
    SampleRate {
        /// New engine sample rate.
        frames: u32,
    },
    /// Sent whenever a new client is registered on the JACK server.
    ///
    /// The corresponding trait method is [`NotificationHandler::client_register`].
    #[doc(alias = "JackClientRegistrationCallback")]
    #[doc(alias = "jack_set_client_registration_callback")]
    ClientRegister {
        /// Name of the client that was registered.
        name: CString,
    },
    /// Sent whenever a client is unregistered on the JACK server.
    ///
    /// The corresponding trait method is [`NotificationHandler::client_unregister`].
    #[doc(alias = "JackClientRegistrationCallback")]
    #[doc(alias = "jack_set_client_registration_callback")]
    ClientUnregister {
        /// Name of the client that was unregistered.
        name: CString,
    },
    /// Sent whenever a new port is registered on the JACK server.
    ///
    /// The corresponding trait method is [`NotificationHandler::port_register`].
    #[doc(alias = "JackPortRegistrationCallback")]
    #[doc(alias = "jack_set_port_registration_callback")]
    PortRegister {
        /// The ID of the port.
        port: PortId,
    },
    /// Sent whenever a port is unregistered on the JACK server.
    ///
    /// The corresponding trait method is [`NotificationHandler::port_unregister`].
    #[doc(alias = "JackPortRegistrationCallback")]
    #[doc(alias = "jack_set_port_registration_callback")]
    PortUnregister {
        /// The ID of the port.
        port: PortId,
    },
    /// Sent whenever a port is connected on the JACK server.
    ///
    /// The corresponding trait method is [`NotificationHandler::port_connect`].
    #[doc(alias = "JackPortConnectCallback")]
    #[doc(alias = "jack_set_port_connect_callback")]
    PortConnect {
        /// The ID of the first port connected.
        a: PortId,
        /// The ID of the second port connected.
        b: PortId,
    },
    /// Sent whenever a port is disconnected on the JACK server.
    ///
    /// The corresponding trait method is [`NotificationHandler::port_disconnect`].
    #[doc(alias = "JackPortConnectCallback")]
    #[doc(alias = "jack_set_port_connect_callback")]
    PortDisconnect {
        /// The ID of the first port disconnected.
        a: PortId,
        /// The ID of the second port disconnected.
        b: PortId,
    },
    /// Sent whenever a port name changes on the JACK server.
    ///
    /// The corresponding trait method is [`NotificationHandler::port_rename`].
    #[doc(alias = "JackPortRenameCallback")]
    #[doc(alias = "jack_set_port_rename_callback")]
    PortRename {
        /// The ID of the renamed port.
        port: PortId,
        /// The previous name of the port.
        old_name: CString,
        /// The new name of the port.
        new_name: CString,
    },
    /// Sent whenever the processing graph is reordered on the JACK server.
    ///
    /// The corresponding trait method is [`NotificationHandler::graph_order`].
    #[doc(alias = "JackGraphOrderCallback")]
    #[doc(alias = "jack_set_graph_order_callback")]
    GraphOrder,
    /// Sent whenever an xrun has occurred.
    ///
    /// The corresponding trait method is [`NotificationHandler::xrun`].
    ///
    /// See also [`Client::xrun_delayed_usecs`](crate::Client::xrun_delayed_usecs).
    #[doc(alias = "JackXRunCallback")]
    #[doc(alias = "jack_set_xrun_callback")]
    XRun,
    /// Sent by the engine when port latencies need to be recalculated.
    ///
    /// In order to receive this message, you **must** pass
    /// [`ActivateFlags::LATENCY`](crate::ActivateFlags::LATENCY) when activating the client.
    /// Clients that do not call that method will never receive this notification.
    ///
    /// This notification will be sent whenever it is necessary to recompute the latencies for some
    /// or all JACK ports. It will be called twice each time it is needed, once being passed
    /// [`LatencyMode::Capture`] and once [`LatencyMode::Playback`].
    ///
    /// **IMPORTANT**: Most JACK clients do *not* need to register a latency callback.
    ///
    /// Clients that meet any of the following conditions do *not* need to register a latency
    /// callback:
    ///
    /// - They have only input ports.
    /// - They have only output ports.
    /// - Their output is totally unrelated to their input.
    /// - Their output is not delayed relative to their input (i.e. data that arrives in a given
    ///   [`ProcessHandler::process`] is processed and output again in the same callback).
    ///
    /// Clients *not* registering a latency callback *must* also satisfy this condition:
    ///
    /// - They have no multiple distinct internal signal pathways.
    ///
    /// This means that if your client has more than 1 input and output port, and considers them
    /// always "correlated" (e.g. as a stereo pair), then there is only 1 (e.g. stereo) signal
    /// pathway through the client. This would be true, for example, of a stereo FX rack client
    /// that has a left/right input pair and a left/right output pair.
    ///
    /// However, this is somewhat a matter of perspective. The same FX rack client could be
    /// connected so that its two input ports were connected to entirely separate sources. Under
    /// these conditions, the fact that the client does not register a latency callback *may*
    /// result in port latency values being incorrect.
    ///
    /// Clients that do not meet any of those conditions *should* handle this notification, and
    /// use [`ActivateFlags::LATENCY`](crate::ActivateFlags::LATENCY).
    ///
    /// Another case is when a client wants to use
    /// [`Port::latency_range`](crate::Port::latency_range), which only returns meaningful values
    /// when ports get connected and latency values change.
    ///
    /// See the documentation for [`Port::set_latency_range`](crate::Port::set_latency_range) on
    /// how the callback should operate. Remember that the `mode` field in this notification will
    /// need to be passed.
    ///
    /// The corresponding trait method is [`NotificationHandler::latency`].
    #[doc(alias = "JackLatencyCallback")]
    #[doc(alias = "jack_set_latency_callback")]
    Latency {
        /// The mode that latencies should be recalculated for.
        mode: LatencyMode,
    },
    /// Sent by the engine anytime a metadata property or properties have been modified.
    ///
    /// Note that when the key is `None`, it means all properties have been modified. This is often
    /// used to indicate that the removal of all keys.
    ///
    /// The corresponding trait method is [`NotificationHandler::property_change`].
    #[doc(alias = "JackPropertyChangeCallback")]
    #[doc(alias = "jack_set_property_change_callback")]
    PropertyChange {
        /// The subject the change relates to, this can be either a client or port.
        subject: Uuid,
        /// The key of the modified property (URI string).
        key: Option<CString>,
        /// Whether the key has been created, changed or deleted.
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
    /// Invoked whenever the JACK server is shutdown.
    ///
    /// See [`Notification::Shutdown`].
    #[doc(alias = "JackShutdownCallback")]
    #[doc(alias = "jack_on_shutdown")]
    #[inline]
    fn shutdown(&mut self) {}
    /// Invoked whenever the JACK server starts or stops freewheeling.
    ///
    /// See [`Notification::Freewheel`].
    #[doc(alias = "JackFreewheelCallback")]
    #[doc(alias = "jack_set_freewheel_callback")]
    #[inline]
    fn freewheel(&mut self, enabled: bool) {}
    /// Invoked when the client buffer size changes.
    ///
    /// See [`Notification::BufferSize`].
    #[doc(alias = "JackBufferSizeCallback")]
    #[doc(alias = "jack_set_buffer_size_callback")]
    #[inline]
    fn buffer_size(&mut self, frames: u32) {}
    /// Invoked when the engine sample rate changes.
    ///
    /// See [`Notification::SampleRate`].
    #[doc(alias = "JackSampleRateCallback")]
    #[doc(alias = "jack_set_sample_rate_callback")]
    #[inline]
    fn sample_rate(&mut self, frames: u32) {}
    /// Invoked whenever a new client is registered on the JACK server.
    ///
    /// See [`Notification::ClientRegister`].
    #[doc(alias = "JackClientRegistrationCallback")]
    #[doc(alias = "jack_set_client_registration_callback")]
    #[inline]
    fn client_register(&mut self, name: CString) {}
    /// Invoked whenever a client is unregistered on the JACK server.
    ///
    /// See [`Notification::ClientUnregister`].
    #[doc(alias = "JackClientRegistrationCallback")]
    #[doc(alias = "jack_set_client_registration_callback")]
    #[inline]
    fn client_unregister(&mut self, name: CString) {}
    /// Invoked whenever a new port is registered on the JACK server.
    ///
    /// See [`Notification::PortRegister`].
    #[doc(alias = "JackPortRegistrationCallback")]
    #[doc(alias = "jack_set_port_registration_callback")]
    #[inline]
    fn port_register(&mut self, port: PortId) {}
    /// Invoked whenever a port is unregistered on the JACK server.
    ///
    /// See [`Notification::PortUnregister`].
    #[doc(alias = "JackPortRegistrationCallback")]
    #[doc(alias = "jack_set_port_registration_callback")]
    #[inline]
    fn port_unregister(&mut self, port: PortId) {}
    /// Invoked whenever a port is connected on the JACK server.
    ///
    /// See [`Notification::PortConnect`].
    #[doc(alias = "JackPortConnectCallback")]
    #[doc(alias = "jack_set_port_connect_callback")]
    #[inline]
    fn port_connect(&mut self, a: PortId, b: PortId) {}
    /// Invoked whenever a port is disconnected on the JACK server.
    ///
    /// See [`Notification::PortDisconnect`].
    #[doc(alias = "JackPortConnectCallback")]
    #[doc(alias = "jack_set_port_connect_callback")]
    #[inline]
    fn port_disconnect(&mut self, a: PortId, b: PortId) {}
    /// Invoked whenever a port name changes on the JACK server.
    ///
    /// See [`Notification::PortRename`].
    #[doc(alias = "JackPortRenameCallback")]
    #[doc(alias = "jack_set_port_rename_callback")]
    #[inline]
    fn port_rename(&mut self, port: PortId, old_name: CString, new_name: CString) {}
    /// Invoked whenever the processing graph is reordered on the JACK server.
    ///
    /// See [`Notification::GraphOrder`].
    #[doc(alias = "JackGraphOrderCallback")]
    #[doc(alias = "jack_set_graph_order_callback")]
    #[inline]
    fn graph_order(&mut self) {}
    /// Invoked whenever an xrun has occurred.
    ///
    /// See [`Notification::XRun`].
    #[doc(alias = "JackXRunCallback")]
    #[doc(alias = "jack_set_xrun_callback")]
    #[inline]
    fn xrun(&mut self) {}
    /// Invoked when port latencies need to be recalculated.
    ///
    /// See [`Notification::Latency`].
    #[doc(alias = "JackLatencyCallback")]
    #[doc(alias = "jack_set_latency_callback")]
    #[inline]
    fn latency(&mut self, mode: LatencyMode) {}
    /// Invoked anytime a metadata property or properties have been modified.
    ///
    /// See [`Notification::PropertyChange`].
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

/// The latency mode currently being computed by the latency callback.
///
/// The purpose of JACK's latency API is to allow clients to easily answer two questions:
///
/// - How long has it been since the data read from a port arrived at the edge of the JACK graph
///   (either via a physical port or being synthesized from scratch)?
/// - How long will it be before the data written to a port arrives at the edge of a JACK graph?
///
/// To help answering these two questions, all JACK ports have two latency values associated with
/// them, both measured in frames.
///
/// Both latencies might potentially have more than one value because there may be multiple
/// pathways to/from a given port and a terminal port. Latency is therefore generally expressed a
/// min/max pair.
///
/// In most common setups, the minimum and maximum latency are the same, but this design
/// accommodates more complex routing, and allows applications (and thus users) to detect cases
/// where routing is creating an anomalous situation that may either need fixing or more
/// sophisticated handling by clients that care about latency.
///
/// See also [`Notification::Latency`] for details on how clients that add latency to the signal
/// path should interact with JACK to ensure that the correct latency figures are used.
#[doc(alias = "JackLatencyCallbackMode")]
#[doc(alias = "jack_latency_callback_mode_t")]
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub enum LatencyMode {
    /// Latency for capture (signal input).
    ///
    /// Measures how long since the data read from the buffer of a port arrived at a port marked
    /// with [`PortFlags::is_terminal`](crate::PortFlags::is_terminal). The data will have come
    /// from the "outside world" if the terminal port is also marked with
    /// [`PortFlags::is_physical`](crate::PortFlags::is_physical), or will have been synthesized by
    /// the client that owns the terminal port.
    #[doc(alias = "JackCaptureLatency")]
    Capture,
    /// Latency for playback (signal output).
    ///
    /// Measures how long until the data written to the buffer of port will reach a port marked
    /// with [`PortFlags::is_terminal`](crate::PortFlags::is_terminal).
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

/// Sub-type of property change notifications.
#[doc(alias = "jack_property_change_t")]
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub enum PropertyChange {
    /// A property was newly created.
    #[doc(alias = "PropertyCreated")]
    Created,
    /// An exist property had its value changed.
    #[doc(alias = "PropertyChanged")]
    Changed,
    /// An existing property was deleted.
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
