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
    #[doc(alias = "JackOptions")]
    #[doc(alias = "jack_options_t")]
    #[derive(Default)]
    pub struct ClientOptions: sys::JackOptions {
        #[doc(alias = "JackNoStartServer")]
        const NO_START_SERVER = sys::JackOptions_JackNoStartServer;
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

pub trait MainThreadContext {
    fn spawn_local<F: std::future::Future<Output = ()> + 'static>(&self, fut: F);
    type IntervalStream: futures_core::Stream<Item = ()> + 'static;
    fn interval(&self, period: Duration) -> Self::IntervalStream;
}

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
    #[inline]
    pub fn with_context(client_name: &str, context: Context) -> Self {
        Self {
            name: client_name.to_owned(),
            flags: sys::JackOptions_JackNullOption,
            server_name: None,
            context,
        }
    }
    #[inline]
    pub fn flags(mut self, flags: ClientOptions) -> Self {
        self.flags |= flags.bits();
        self
    }
    #[doc(alias = "JackServerName")]
    #[inline]
    pub fn server_name(mut self, server_name: impl Into<CString>) -> Self {
        self.server_name = Some(server_name.into());
        self.flags |= sys::JackOptions_JackServerName;
        self
    }
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
    #[doc(alias = "JackPortFlags")]
    #[derive(Default)]
    pub struct PortCreateFlags: sys::JackPortFlags {
        #[doc(alias = "JackPortIsPhysical")]
        const PHYSICAL    = sys::JackPortFlags_JackPortIsPhysical;
        #[doc(alias = "JackPortCanMonitor")]
        const CAN_MONITOR = sys::JackPortFlags_JackPortCanMonitor;
        #[doc(alias = "JackPortIsTerminal")]
        const TERMINAL = sys::JackPortFlags_JackPortIsTerminal;
    }
}

#[doc(alias = "JackStatus")]
#[doc(alias = "jack_status_t")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(transparent)]
pub struct Status(sys::jack_status_t);

impl Status {
    #[doc(alias = "JackNameNotUnique")]
    #[inline]
    pub fn has_exact_name(&self) -> bool {
        (self.0 & sys::JackStatus_JackNameNotUnique) == 0
    }
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
    #[derive(Default)]
    pub struct ActivateFlags: u32 {
        #[doc(alias = "jack_set_latency_callback")]
        const LATENCY        = 1 << 0;
        #[doc(alias = "jack_set_sync_callback")]
        const SYNC           = 1 << 1;
        #[doc(alias = "jack_set_timebase_callback")]
        const TIMEBASE       = 1 << 2;
        #[doc(alias = "jack_set_timebase_callback")]
        const TIMEBASE_FORCE = (1 << 3) | Self::TIMEBASE.bits;
    }
}

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
    #[doc(alias = "jack_set_buffer_size")]
    pub fn set_buffer_size(&self, nframes: u32) -> crate::Result<()> {
        Error::check_ret(unsafe {
            self.client
                .lib
                .jack_set_buffer_size(self.client.as_ptr(), nframes)
        })
    }
    pub fn buffer_size(&self) -> u32 {
        unsafe { self.client.lib.jack_get_buffer_size(self.client.as_ptr()) }
    }
    #[doc(alias = "jack_get_sample_rate")]
    pub fn sample_rate(&self) -> u32 {
        unsafe { self.client.lib.jack_get_sample_rate(self.client.as_ptr()) }
    }
    #[doc(alias = "jack_cpu_load")]
    pub fn cpu_load(&self) -> f32 {
        unsafe { self.client.lib.jack_cpu_load(self.client.as_ptr()) }
    }
    pub fn frame_duration(&self) -> Duration {
        let secs = self.buffer_size() as f64 / self.sample_rate() as f64;
        std::time::Duration::from_secs_f64(secs)
    }
    #[doc(alias = "jack_frame_time")]
    pub fn frame_time(&self) -> Frames {
        Frames(unsafe { self.client.lib.jack_frame_time(self.client.as_ptr()) })
    }
    #[doc(alias = "jack_frames_since_cycle_start")]
    pub fn frames_since_cycle_start(&self) -> Frames {
        Frames(unsafe {
            self.client
                .lib
                .jack_frames_since_cycle_start(self.client.as_ptr())
        })
    }
    #[doc(alias = "jack_frames_to_time")]
    pub fn frames_to_time(&self, frames: Frames) -> Time {
        Time(unsafe {
            self.client
                .lib
                .jack_frames_to_time(self.client.as_ptr(), frames.0)
        })
    }
    #[doc(alias = "jack_time_to_frames")]
    pub fn time_to_frames(&self, time: Time) -> Frames {
        Frames(unsafe {
            self.client
                .lib
                .jack_time_to_frames(self.client.as_ptr(), time.0)
        })
    }
    #[doc(alias = "jack_set_freewheel")]
    pub fn set_freewheel(&self, enabled: bool) -> crate::Result<()> {
        Error::check_ret(unsafe {
            self.client
                .lib
                .jack_set_freewheel(self.client.as_ptr(), enabled as _)
        })
    }
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
    pub fn port_by_owned_uuid(&self, port: OwnedPortUuid) -> Option<Port> {
        self.ports
            .borrow()
            .get(&port)
            .and_then(|p| Port::new(&self.client, p.0.ptr.port.as_ptr()))
    }
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
    #[doc(alias = "jack_port_is_mine")]
    pub fn port_is_mine(&self, port: Port) -> bool {
        (unsafe {
            self.client
                .lib
                .jack_port_is_mine(self.client.client.as_ptr(), port.as_ptr().as_ptr())
        }) == 1
    }
    #[doc(alias = "jack_port_get_all_connections")]
    pub fn port_get_all_connections(&self, port: Port) -> Vec<CString> {
        PortList(unsafe {
            self.client
                .lib
                .jack_port_get_all_connections(self.client.client.as_ptr(), port.as_ptr().as_ptr())
        })
        .to_vec()
    }
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
    #[doc(alias = "jack_recompute_total_latencies")]
    pub fn recompute_total_latencies(&self) -> crate::Result<()> {
        Error::check_ret(unsafe {
            self.client
                .lib
                .jack_recompute_total_latencies(self.client.as_ptr())
        })
    }
    #[doc(alias = "jack_set_sync_timeout")]
    pub fn set_sync_timeout(&self, timeout: Time) -> crate::Result<()> {
        Error::check_ret(unsafe {
            self.client
                .lib
                .jack_set_sync_timeout(self.client.as_ptr(), timeout.as_micros())
        })
    }
    #[inline]
    pub fn transport(&self) -> Transport {
        Transport::new(&self.client)
    }
    #[doc(alias = "jack_get_client_name")]
    pub fn name(&self) -> CString {
        unsafe {
            let ptr = self.client.lib.jack_get_client_name(self.client.as_ptr());
            crate::JackStr::from_raw_unchecked(ptr).as_cstring()
        }
    }
    #[doc(alias = "jack_client_get_uuid")]
    pub fn uuid(&self) -> Uuid {
        unsafe {
            Uuid::from_raw(self.client.lib.jack_client_get_uuid(self.client.as_ptr())).unwrap()
        }
    }
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
    #[doc(alias = "jack_remove_all_properties")]
    pub fn remove_all_properties(&self) -> crate::Result<()> {
        Error::check_ret(unsafe {
            self.client
                .lib
                .jack_remove_all_properties(self.client.as_ptr())
        })
    }
    #[doc(alias = "jack_get_max_delayed_usecs")]
    pub fn max_delayed_usecs(&self) -> f32 {
        unsafe {
            self.client
                .lib
                .jack_get_max_delayed_usecs(self.client.as_ptr())
        }
    }
    #[doc(alias = "jack_get_xrun_delayed_usecs")]
    pub fn xrun_delayed_usecs(&self) -> f32 {
        unsafe {
            self.client
                .lib
                .jack_get_xrun_delayed_usecs(self.client.as_ptr())
        }
    }
    #[doc(alias = "jack_reset_max_delayed_usecs")]
    pub fn reset_max_delayed_usecs(&self) {
        unsafe {
            self.client
                .lib
                .jack_reset_max_delayed_usecs(self.client.as_ptr());
        }
    }
    #[inline]
    pub fn as_ptr(&self) -> NonNull<sys::jack_client_t> {
        self.client.client
    }
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

pub struct ActivateError<Context, Process, Notify>
where
    Context: MainThreadContext,
    Process: ProcessHandler,
    Notify: NotificationHandler,
{
    pub client: InactiveClient<Context, Process::PortData>,
    pub process_handler: Option<Process>,
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

pub struct DeactivateError<Context, Process, Notify>
where
    Context: MainThreadContext,
    Process: ProcessHandler,
    Notify: NotificationHandler,
{
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
    #[doc(alias = "jack_set_sync_callback")]
    pub fn register_sync_callback(&self) -> crate::Result<()> {
        self.process_data
            .as_ref()
            .ok_or_else(Error::invalid_option)
            .and_then(|process_data| unsafe {
                crate::set_sync_callback(&self.client.client, &**process_data.get())
            })
    }
    #[doc(alias = "jack_set_sync_callback")]
    pub fn unregister_sync_callback(&self) -> crate::Result<()> {
        unsafe { crate::unset_sync_callback(&self.client.client) }
    }
    #[doc(alias = "jack_set_timebase_callback")]
    pub fn acquire_timebase(&self, force: bool) -> crate::Result<()> {
        self.process_data
            .as_ref()
            .ok_or_else(Error::invalid_option)
            .and_then(|process_data| unsafe {
                crate::set_timebase_callback(&self.client.client, !force, &**process_data.get())
            })
    }
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

#[derive(Clone, Debug)]
pub struct PortInfo<'s, PortData: Send + 'static> {
    pub name: &'s str,
    pub type_: PortType,
    pub mode: PortMode,
    pub flags: PortCreateFlags,
    pub data: PortData,
}

impl<'s, PortData: Send + 'static> PortInfo<'s, PortData> {
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
    #[inline]
    pub fn flags(mut self, flags: PortCreateFlags) -> Self {
        self.flags = flags;
        self
    }
}

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
