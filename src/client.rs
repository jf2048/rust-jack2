use atomic_triple_buffer::AtomicTripleBuffer;
use std::{
    cell::{RefCell, UnsafeCell},
    collections::HashMap,
    ffi::{CStr, CString},
    num::NonZeroU64,
    ptr::NonNull,
    rc::Rc,
    sync::{atomic::AtomicU32, Arc},
    time::Duration,
};

use crate::{
    callbacks::*, sys, Error, Failure, Frames, Port, PortFlags, PortId, PortList, PortMode,
    PortPtr, PortType, ProcessPort, ProcessPorts, Time, Transport, Uuid,
};

#[derive(Clone)]
pub(crate) struct ClientHandle {
    pub lib: &'static sys::Jack,
    pub client: NonNull<sys::jack_client_t>,
}

impl std::fmt::Debug for ClientHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClientHandle")
            .field("client", &self.client)
            .finish()
    }
}

impl ClientHandle {
    pub fn as_ptr(&self) -> *mut sys::jack_client_t {
        self.client.as_ptr()
    }
}

bitflags::bitflags! {
    pub struct ClientOptions: sys::JackOptions {
        const NO_START_SERVER = sys::JackOptions_JackNoStartServer;
        const USE_EXACT_NAME  = sys::JackOptions_JackUseExactName;
    }
}

#[derive(Debug)]
#[must_use]
pub struct ClientBuilder<Process = (), Notify = ()>
where
    Process: ProcessHandler,
    Notify: NotificationHandler,
{
    name: String,
    flags: sys::jack_options_t,
    server_name: Option<CString>,
    process: Option<Process>,
    notification: Option<Notify>,
    context: Option<glib::MainContext>,
}

impl<Process, Notify> ClientBuilder<Process, Notify>
where
    Process: ProcessHandler,
    Notify: NotificationHandler,
{
    #[inline]
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            flags: sys::JackOptions_JackNullOption,
            server_name: None,
            process: None,
            notification: None,
            context: None,
        }
    }
    #[inline]
    pub fn flags(&mut self, flags: ClientOptions) -> &mut Self {
        self.flags |= flags.bits();
        self
    }
    #[inline]
    pub fn server_name(&mut self, server_name: impl Into<CString>) -> &mut Self {
        self.server_name = Some(server_name.into());
        self.flags |= sys::JackOptions_JackServerName;
        self
    }
    #[inline]
    pub fn process_handler(&mut self, process: Process) -> &mut Self {
        self.process = Some(process);
        self
    }
    #[inline]
    pub fn notification_handler(&mut self, notification: Notify) -> &mut Self {
        self.notification = Some(notification);
        self
    }
    #[inline]
    pub fn context(&mut self, context: &glib::MainContext) -> &mut Self {
        self.context = Some(context.clone());
        self
    }
    pub fn build(self) -> crate::Result<Client<Process>> {
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
            process,
            notification,
            context,
            ..
        } = self;

        let name = CString::new(name)?;
        let context = context.unwrap_or_else(glib::MainContext::ref_thread_default);

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

        struct ClientTmp(ClientHandle);
        impl Drop for ClientTmp {
            fn drop(&mut self) {
                unsafe {
                    self.0.lib.jack_client_close(self.0.client.as_ptr());
                }
            }
        }
        let tmp = ClientTmp(client.clone());

        let buffer_size = Arc::new(AtomicU32::new(unsafe {
            client.lib.jack_get_buffer_size(client.client.as_ptr())
        }));
        let process_ports = Arc::new(AtomicTripleBuffer::default());

        let process_data = if let Some(process) = process {
            let process_data = Box::new(ProcessData {
                client: client.clone(),
                buffer_size: buffer_size.clone(),
                ports: process_ports.clone(),
                handler: process,
            });
            unsafe {
                crate::set_process_callbacks(&client, &*process_data)?;
            }
            Some(UnsafeCell::new(process_data))
        } else {
            None
        };

        let notification = if let Some(mut notification) = notification {
            let (tx, mut rx) = futures_channel::mpsc::unbounded();
            let notification_data = Box::new(NotificationData { tx });
            unsafe {
                crate::set_notification_callbacks(&client, &*notification_data)?;
            }
            context.spawn_local(async move {
                use futures_util::StreamExt;
                while let Some(msg) = rx.next().await {
                    notification.notification(msg);
                }
            });
            Some(UnsafeCell::new(notification_data))
        } else {
            None
        };

        std::mem::forget(tmp);

        Ok(Client {
            client,
            buffer_size,
            process_data,
            notification,
            ports: Default::default(),
            process_ports,
            context,
            port_gc_source: Default::default(),
        })
    }
}

bitflags::bitflags! {
    pub struct PortCreateFlags: sys::JackPortFlags {
        const PHYSICAL    = sys::JackPortFlags_JackPortIsPhysical;
        const CAN_MONITOR = sys::JackPortFlags_JackPortCanMonitor;
        const IS_TERMINAL = sys::JackPortFlags_JackPortIsTerminal;
    }
}

#[derive(Debug)]
pub struct Client<Process = ()>
where
    Process: ProcessHandler,
{
    client: ClientHandle,
    buffer_size: Arc<AtomicU32>,
    process_data: Option<UnsafeCell<Box<ProcessData<Process>>>>,
    notification: Option<UnsafeCell<Box<NotificationData>>>,
    ports: RefCell<HashMap<OwnedPort, ProcessPort<Process::PortData>>>,
    process_ports: Arc<AtomicTripleBuffer<ProcessPorts<Process::PortData>>>,
    context: glib::MainContext,
    port_gc_source: Rc<SourceCell>,
}

#[derive(Default)]
#[repr(transparent)]
struct SourceCell(std::cell::Cell<Option<glib::SourceId>>);

impl std::ops::Deref for SourceCell {
    type Target = std::cell::Cell<Option<glib::SourceId>>;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Drop for SourceCell {
    #[inline]
    fn drop(&mut self) {
        if let Some(source) = self.0.take() {
            source.remove();
        }
    }
}

impl std::fmt::Debug for SourceCell {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let source = self.0.take();
        let id = source.as_ref().map(|s| unsafe { s.as_raw() });
        self.0.set(source);
        f.debug_tuple("SourceCell").field(&id).finish()
    }
}

impl<Process> Client<Process>
where
    Process: ProcessHandler,
{
    #[inline]
    pub fn as_ptr(&self) -> NonNull<sys::jack_client_t> {
        self.client.client
    }
    pub fn activate(&self) -> crate::Result<()> {
        unsafe {
            Error::check_ret(self.client.lib.jack_activate(self.client.as_ptr()))?;
        }
        Ok(())
    }
    pub fn deactivate(&self) -> crate::Result<()> {
        unsafe {
            Error::check_ret(self.client.lib.jack_deactivate(self.client.as_ptr()))?;
        }
        Ok(())
    }
    pub fn set_buffer_size(&self, nframes: u32) -> crate::Result<()> {
        Error::check_ret(unsafe {
            self.client
                .lib
                .jack_set_buffer_size(self.client.as_ptr(), nframes)
        })?;
        self.buffer_size
            .store(nframes, std::sync::atomic::Ordering::Release);
        Ok(())
    }
    pub fn buffer_size(&self) -> u32 {
        self.buffer_size.load(std::sync::atomic::Ordering::Acquire)
    }
    pub fn sample_rate(&self) -> u32 {
        unsafe { self.client.lib.jack_get_sample_rate(self.client.as_ptr()) }
    }
    pub fn cpu_load(&self) -> f32 {
        unsafe { self.client.lib.jack_cpu_load(self.client.as_ptr()) }
    }
    pub fn frame_duration(&self) -> Duration {
        let secs = self.buffer_size() as f64 / self.sample_rate() as f64;
        std::time::Duration::from_secs_f64(secs)
    }
    pub fn frame_time(&self) -> u32 {
        unsafe { self.client.lib.jack_frame_time(self.client.as_ptr()) }
    }
    pub fn frames_since_cycle_start(&self) -> Frames {
        Frames(unsafe {
            self.client
                .lib
                .jack_frames_since_cycle_start(self.client.as_ptr())
        })
    }
    pub fn frames_to_time(&self, frames: Frames) -> Time {
        Time(unsafe {
            self.client
                .lib
                .jack_frames_to_time(self.client.as_ptr(), frames.0)
        })
    }
    pub fn time_to_frames(&self, time: Time) -> Frames {
        Frames(unsafe {
            self.client
                .lib
                .jack_time_to_frames(self.client.as_ptr(), time.0)
        })
    }
    pub fn set_freewheel(&self, enabled: bool) -> crate::Result<()> {
        Error::check_ret(unsafe {
            self.client
                .lib
                .jack_set_freewheel(self.client.as_ptr(), enabled as _)
        })
    }
    pub fn register_ports<'p>(
        &self,
        ports: impl IntoIterator<Item = PortDef<'p, Process::PortData>>,
    ) -> Vec<Option<OwnedPort>> {
        let mut ports = ports
            .into_iter()
            .map(
                |PortDef {
                     name,
                     type_,
                     mode,
                     flags,
                     data,
                 }| {
                    let port_name = CString::new(name).ok()?;
                    let flags = mode.flags() | flags.bits();

                    unsafe {
                        let port = self.client.lib.jack_port_register(
                            self.client.as_ptr(),
                            port_name.as_ptr(),
                            type_.as_cstr().as_ptr(),
                            flags.into(),
                            0,
                        );
                        let ptr = PortPtr::new(
                            &self.client,
                            port,
                            PortFlags::from_bits_unchecked(flags),
                            type_,
                        )?;
                        let port = OwnedPort::new(&self.client, port)?;
                        Some((port, ptr, data))
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
                            .insert(port, ProcessPort::new(ptr, data));
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
    pub fn unregister_ports(&self, ports: impl IntoIterator<Item = OwnedPort>) {
        let mut port_map = self.ports.borrow_mut();
        // keep PortPtrs around to unregister after clearing buffers
        let removed = ports
            .into_iter()
            .filter_map(|p| port_map.remove(&p))
            .collect::<Vec<_>>();
        drop(port_map);
        if !removed.is_empty() {
            let mut bufs = self.process_ports.back_buffers().unwrap();
            let mut needs_gc = false;
            if let Some(pending) = bufs.pending_mut() {
                pending.ports.clear();
            } else {
                needs_gc = true;
            }
            bufs.back_mut().ports.clear();
            drop(bufs);
            if needs_gc {
                self.defer_port_gc();
            }
            drop(removed);
        }
    }
    fn defer_port_gc(&self) {
        let id = self.port_gc_source.take().unwrap_or_else(|| {
            // SAFETY
            // the source must be removed when Client is dropped because it holds a
            // reference to process_ports
            let ports = Arc::downgrade(&self.process_ports);
            let source_cell = Rc::downgrade(&self.port_gc_source);
            let guard = glib::thread_guard::ThreadGuard::new((ports, source_cell));
            let source = glib::timeout_source_new(
                self.frame_duration(),
                Some("jack port cleanup"),
                glib::PRIORITY_HIGH,
                move || {
                    let (ports, source_cell) = guard.get_ref();
                    let ports = match ports.upgrade() {
                        Some(ports) => ports,
                        None => return glib::Continue(false),
                    };
                    let mut bufs = ports.back_buffers().unwrap();
                    if let Some(pending) = bufs.pending_mut() {
                        pending.ports.clear();
                        drop(pending);
                        if let Some(source_cell) = source_cell.upgrade() {
                            source_cell.take();
                        }
                        glib::Continue(false)
                    } else {
                        glib::Continue(true)
                    }
                },
            );
            source.attach(Some(&self.context))
        });
        self.port_gc_source.set(Some(id));
    }
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
                flags.bits() as _,
            )
        })
        .to_vec()
    }
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
    pub fn port_is_mine(&self, port: Port) -> bool {
        (unsafe {
            self.client
                .lib
                .jack_port_is_mine(self.client.client.as_ptr(), port.as_ptr().as_ptr())
        }) == 1
    }
    pub fn port_get_all_connections(&self, port: Port) -> Vec<CString> {
        PortList(unsafe {
            self.client
                .lib
                .jack_port_get_all_connections(self.client.client.as_ptr(), port.as_ptr().as_ptr())
        })
        .to_vec()
    }
    pub fn port_rename(&self, port: OwnedPort, port_name: impl AsRef<CStr>) -> crate::Result<()> {
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
    pub fn port_disconnect(&self, port: OwnedPort) -> crate::Result<()> {
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
    pub fn recompute_total_latencies(&self) -> crate::Result<()> {
        Error::check_ret(unsafe {
            self.client
                .lib
                .jack_recompute_total_latencies(self.client.as_ptr())
        })
    }
    pub fn register_latency_callback(&self) -> crate::Result<()> {
        let notification_data = self
            .notification
            .as_ref()
            .ok_or_else(Error::invalid_option)?;
        unsafe { crate::set_latency_callback(&self.client, &**notification_data.get()) }
    }
    pub fn register_sync_callback(&self) -> crate::Result<()> {
        let process_data = self
            .process_data
            .as_ref()
            .ok_or_else(Error::invalid_option)?;
        unsafe { crate::set_sync_callback(&self.client, &**process_data.get()) }
    }
    pub fn unregister_sync_callback(&self) -> crate::Result<()> {
        unsafe { crate::unset_sync_callback(&self.client) }
    }
    pub fn acquire_timebase(&self, conditional: bool) -> std::io::Result<()> {
        let process_data = self.process_data.as_ref().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "no process data")
        })?;
        unsafe { crate::set_timebase_callback(&self.client, conditional, &**process_data.get()) }
    }
    pub fn release_timebase(&self) -> crate::Result<()> {
        Error::check_ret(unsafe { self.client.lib.jack_release_timebase(self.client.as_ptr()) })
    }
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
    pub fn name(&self) -> CString {
        unsafe {
            let ptr = self.client.lib.jack_get_client_name(self.client.as_ptr());
            crate::JackStr::from_raw_unchecked(ptr).as_cstring()
        }
    }
    pub fn uuid(&self) -> Uuid {
        unsafe {
            Uuid::from_raw(self.client.lib.jack_client_get_uuid(self.client.as_ptr())).unwrap()
        }
    }
    pub fn uuid_for_client_name(&self, name: impl AsRef<CStr>) -> Option<Uuid> {
        unsafe {
            Uuid::from_raw(
                self.client
                    .lib
                    .jack_get_uuid_for_client_name(self.client.as_ptr(), name.as_ref().as_ptr()),
            )
        }
    }
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
    pub fn remove_property(&self, subject: Uuid, key: impl AsRef<CStr>) -> crate::Result<()> {
        Error::check_ret(unsafe {
            self.client.lib.jack_remove_property(
                self.client.as_ptr(),
                subject.0.get(),
                key.as_ref().as_ptr(),
            )
        })
    }
    pub fn remove_properties(&self, subject: Uuid) -> crate::Result<()> {
        Error::check_ret(unsafe {
            self.client
                .lib
                .jack_remove_properties(self.client.as_ptr(), subject.0.get())
        })
    }
    pub fn remove_all_properties(&self) -> crate::Result<()> {
        Error::check_ret(unsafe {
            self.client
                .lib
                .jack_remove_all_properties(self.client.as_ptr())
        })
    }
    pub fn max_delayed_usecs(&self) -> f32 {
        unsafe {
            self.client
                .lib
                .jack_get_max_delayed_usecs(self.client.as_ptr())
        }
    }
    pub fn xrun_delayed_usecs(&self) -> f32 {
        unsafe {
            self.client
                .lib
                .jack_get_xrun_delayed_usecs(self.client.as_ptr())
        }
    }
    pub fn reset_max_delayed_usecs(&self) {
        unsafe {
            self.client
                .lib
                .jack_reset_max_delayed_usecs(self.client.as_ptr());
        }
    }
}

impl<Process: ProcessHandler> Drop for Client<Process> {
    fn drop(&mut self) {
        unsafe {
            self.client.lib.jack_client_close(self.client.as_ptr());
        }
    }
}

#[derive(Clone, Debug)]
pub struct PortDef<'s, PortData: Send + 'static> {
    pub name: &'s str,
    pub type_: PortType,
    pub mode: PortMode,
    pub flags: PortCreateFlags,
    pub data: PortData,
}

#[repr(transparent)]
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct OwnedPort(Uuid);

impl OwnedPort {
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

pub(crate) struct ProcessData<P: ProcessHandler> {
    pub client: ClientHandle,
    pub buffer_size: Arc<AtomicU32>,
    pub ports: Arc<AtomicTripleBuffer<ProcessPorts<P::PortData>>>,
    pub handler: P,
}

pub(crate) struct NotificationData {
    pub tx: futures_channel::mpsc::UnboundedSender<Notification>,
}
