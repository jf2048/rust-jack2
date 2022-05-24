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

#[allow(unused)]
pub trait ProcessHandler: Send + 'static {
    type PortData: Send + 'static;
    #[inline]
    fn thread_init(&mut self) {}
    #[inline]
    fn process(&mut self, scope: ProcessScope<Self::PortData>) -> ControlFlow<(), ()> {
        ControlFlow::Continue(())
    }
    #[inline]
    fn buffer_size(&mut self, scope: ProcessScope<Self::PortData>) -> ControlFlow<(), ()> {
        ControlFlow::Continue(())
    }
    #[inline]
    fn sync(
        &mut self,
        state: TransportState,
        pos: &TransportPosition,
        scope: ProcessScope<Self::PortData>,
    ) -> ControlFlow<(), ()> {
        ControlFlow::Continue(())
    }
    #[inline]
    fn timebase(
        &mut self,
        state: TransportState,
        pos: &TransportPosition,
        scope: ProcessScope<Self::PortData>,
    ) {
    }
    #[inline]
    fn new_timebase(
        &mut self,
        state: TransportState,
        pos: &mut TransportPosition,
        scope: ProcessScope<Self::PortData>,
    ) {
    }
}

impl ProcessHandler for () {
    type PortData = ();
}

pub struct ProcessClosureHandler<Func, PortData = ()>(Func, PhantomData<PortData>)
where
    Func: for<'a> FnMut(ProcessScope<'a, PortData>) -> ControlFlow<(), ()> + Send + 'static,
    PortData: Send + 'static;

impl<Func, PortData> ProcessClosureHandler<Func, PortData>
where
    Func: for<'a> FnMut(ProcessScope<'a, PortData>) -> ControlFlow<(), ()> + Send + 'static,
    PortData: Send + 'static,
{
    #[inline]
    pub fn new(func: Func) -> Self {
        Self(func, PhantomData)
    }
}

impl<Func, PortData> ProcessHandler for ProcessClosureHandler<Func, PortData>
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

#[derive(Debug, Clone)]
pub enum Notification {
    Shutdown,
    Freewheel {
        enabled: bool,
    },
    SampleRate {
        frames: u32,
    },
    ClientRegister {
        name: CString,
    },
    ClientUnregister {
        name: CString,
    },
    PortRegister {
        port: PortId,
    },
    PortUnregister {
        port: PortId,
    },
    PortConnect {
        a: PortId,
        b: PortId,
    },
    PortDisconnect {
        a: PortId,
        b: PortId,
    },
    PortRename {
        port: PortId,
        old_name: CString,
        new_name: CString,
    },
    GraphOrder,
    XRun,
    Latency {
        mode: LatencyMode,
    },
    PropertyChange {
        subject: Uuid,
        key: CString,
        change: PropertyChange,
    },
}

#[allow(unused)]
pub trait NotificationHandler: 'static {
    #[inline]
    fn notification(&mut self, msg: Notification) {
        use Notification::*;
        match msg {
            Shutdown => self.shutdown(),
            Freewheel { enabled } => self.freewheel(enabled),
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
    #[inline]
    fn shutdown(&mut self) {}
    #[inline]
    fn freewheel(&mut self, enabled: bool) {}
    #[inline]
    fn sample_rate(&mut self, frames: u32) {}
    #[inline]
    fn client_register(&mut self, name: CString) {}
    #[inline]
    fn client_unregister(&mut self, name: CString) {}
    #[inline]
    fn port_register(&mut self, port: PortId) {}
    #[inline]
    fn port_unregister(&mut self, port: PortId) {}
    #[inline]
    fn port_connect(&mut self, a: PortId, b: PortId) {}
    #[inline]
    fn port_disconnect(&mut self, a: PortId, b: PortId) {}
    #[inline]
    fn port_rename(&mut self, port: PortId, old_name: CString, new_name: CString) {}
    #[inline]
    fn graph_order(&mut self) {}
    #[inline]
    fn xrun(&mut self) {}
    #[inline]
    fn latency(&mut self, mode: LatencyMode) {}
    #[inline]
    fn property_change(&mut self, subject: Uuid, key: CString, change: PropertyChange) {}
}

impl NotificationHandler for () {}

pub struct NotificationClosureHandler<F: FnMut(Notification) + 'static>(F);

impl<F: FnMut(Notification) + 'static> NotificationClosureHandler<F> {
    #[inline]
    pub fn new(func: F) -> Self {
        Self(func)
    }
}

impl<F: FnMut(Notification) + 'static> NotificationHandler for NotificationClosureHandler<F> {
    #[inline]
    fn notification(&mut self, msg: Notification) {
        self.0(msg);
    }
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub enum LatencyMode {
    Capture,
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

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub enum PropertyChange {
    Created,
    Changed,
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

unsafe extern "C" fn buffer_size_callback<P: ProcessHandler>(
    frames: sys::jack_nframes_t,
    arg: *mut c_void,
) -> c_int {
    let data = &mut *(arg as *mut ProcessData<P>);
    data.buffer_size
        .store(frames, std::sync::atomic::Ordering::Release);
    let mut front = data.ports.front_buffer().unwrap();
    let scope = ProcessScope {
        client: &data.client,
        ports: &mut *front,
        nframes: frames,
    };
    match data.handler.buffer_size(scope) {
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
    Error::check_ret(client.lib.jack_set_buffer_size_callback(
        client.as_ptr(),
        Some(buffer_size_callback::<P>),
        process as *mut _,
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
            key: CStr::from_ptr(key).into(),
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
    if new_pos == 0 {
        let pos = &*(pos as *mut TransportPosition);
        data.handler.timebase(state, pos, scope);
    } else {
        let pos = &mut *(pos as *mut TransportPosition);
        data.handler.new_timebase(state, pos, scope);
    }
}

#[inline]
pub(crate) unsafe fn set_timebase_callback<P: ProcessHandler>(
    client: &ClientHandle,
    conditional: bool,
    process: *const ProcessData<P>,
) -> std::io::Result<()> {
    let ret = client.lib.jack_set_timebase_callback(
        client.as_ptr(),
        conditional as _,
        Some(timebase_callback::<P>),
        process as *mut _,
    );
    if ret < 0 {
        return Err(std::io::Error::from_raw_os_error(-ret));
    }
    Ok(())
}
