use std::{
    cell::{RefCell, RefMut},
    collections::HashMap,
    ffi::c_void,
    marker::PhantomData,
    mem::MaybeUninit,
    num::NonZeroU64,
    ptr::NonNull,
    rc::Rc,
    time::Duration,
};

use crate::{
    sys::{self, library},
    ClientHandle, Error, Frames, OwnedPortUuid, PortFlags, PortType, Time, Transport, Uuid,
};

#[derive(Debug)]
pub struct ProcessScope<'scope, PortData> {
    pub(crate) client: &'scope ClientHandle,
    pub(crate) ports: &'scope mut ProcessPorts<PortData>,
    pub(crate) nframes: u32,
}

impl<'scope, PortData> ProcessScope<'scope, PortData> {
    #[inline]
    pub fn nframes(&self) -> u32 {
        self.nframes
    }
    #[doc(alias = "jack_last_frame_time")]
    #[inline]
    pub fn last_frame_time(&self) -> Frames {
        Frames(unsafe { self.client.lib.jack_last_frame_time(self.client.as_ptr()) })
    }
    #[doc(alias = "jack_get_sample_rate")]
    pub fn sample_rate(&self) -> u32 {
        unsafe { self.client.lib.jack_get_sample_rate(self.client.as_ptr()) }
    }
    #[doc(alias = "jack_get_cycle_times")]
    #[inline]
    pub fn cycle_times(&self) -> crate::Result<CycleTimes> {
        let mut current_frames = 0;
        let mut current_usecs = 0;
        let mut next_usecs = 0;
        let mut period_usecs = 0.;
        let ret = unsafe {
            self.client.lib.jack_get_cycle_times(
                self.client.as_ptr(),
                &mut current_frames,
                &mut current_usecs,
                &mut next_usecs,
                &mut period_usecs,
            )
        };
        Error::check_ret(ret)?;
        Ok(CycleTimes {
            current_frames: Frames(current_frames),
            current_time: Time(current_usecs),
            next_time: Time(next_usecs),
            period: Duration::from_secs_f64(period_usecs as f64 * 1_000_000.),
        })
    }
    #[inline]
    pub fn transport(&self) -> Transport {
        Transport::new(self.client)
    }
    #[inline]
    pub fn ports(&'scope self) -> impl Iterator<Item = ProcessPort<'scope, PortData>> + '_ {
        self.ports.ports.values().map(|port| ProcessPort {
            port: &*port.0,
            scope: self,
        })
    }
    #[inline]
    pub fn port_by_owned_uuid(
        &'scope self,
        port: OwnedPortUuid,
    ) -> Option<ProcessPort<'scope, PortData>> {
        self.ports.ports.get(&port).map(|port| ProcessPort {
            port: &*port.0,
            scope: self,
        })
    }
    #[inline]
    pub fn client_ptr(&self) -> NonNull<sys::jack_client_t> {
        self.client.client
    }
    #[inline]
    pub fn library(&self) -> &sys::Jack {
        self.client.lib
    }
}

#[derive(Debug)]
pub struct ProcessPort<'scope, PortData> {
    port: &'scope ProcessPortInner<PortData>,
    scope: &'scope ProcessScope<'scope, PortData>,
}

impl<'scope, PortData> ProcessPort<'scope, PortData> {
    #[doc(alias = "jack_port_uuid")]
    pub fn uuid(&self) -> Uuid {
        let uuid = unsafe {
            NonZeroU64::new_unchecked(self.scope.client.lib.jack_port_uuid(self.as_ptr().as_ptr()))
        };
        Uuid(uuid)
    }
    #[doc(alias = "jack_port_type_id")]
    #[inline]
    pub fn type_id(&self) -> PortType {
        self.port.ptr.port_type
    }
    #[doc(alias = "jack_port_flags")]
    #[inline]
    pub fn flags(&self) -> PortFlags {
        self.port.ptr.flags
    }
    #[inline]
    pub fn data(&self) -> &RefCell<PortData> {
        &self.port.data
    }
    #[doc(alias = "jack_port_get_buffer")]
    pub fn audio_in_buffer(&self) -> &[f32] {
        assert!(self.port.ptr.flags.is_input() && self.port.ptr.port_type == PortType::Audio);
        unsafe {
            let buf = self
                .scope
                .client
                .lib
                .jack_port_get_buffer(self.as_ptr().as_ptr(), self.scope.nframes);
            std::slice::from_raw_parts(buf as *const f32, self.scope.nframes as usize)
        }
    }
    #[doc(alias = "jack_port_get_buffer")]
    pub fn audio_out_buffer(&self) -> RefMut<[f32]> {
        assert!(self.port.ptr.flags.is_output() && self.port.ptr.port_type == PortType::Audio);
        let refmut = self.port.buffer_lock.borrow_mut();
        let slice = unsafe {
            let buf = self
                .scope
                .client
                .lib
                .jack_port_get_buffer(self.as_ptr().as_ptr(), self.scope.nframes);
            std::slice::from_raw_parts_mut(buf as *mut f32, self.scope.nframes as usize)
        };
        RefMut::map(refmut, |_| slice)
    }
    #[doc(alias = "jack_port_get_buffer")]
    pub fn midi_in_buffer(&self) -> MidiInput {
        assert!(self.port.ptr.flags.is_input() && self.port.ptr.port_type == PortType::Midi);
        let port_buffer = unsafe {
            NonNull::new_unchecked(
                self.scope
                    .client
                    .lib
                    .jack_port_get_buffer(self.as_ptr().as_ptr(), self.scope.nframes),
            )
        };
        MidiInput {
            port_buffer,
            client: PhantomData,
        }
    }
    #[doc(alias = "jack_port_get_buffer")]
    pub fn midi_out_buffer(&self) -> RefMut<MidiWriter> {
        assert!(self.port.ptr.flags.is_output() && self.port.ptr.port_type == PortType::Midi);
        let refmut = self.port.buffer_lock.borrow_mut();
        let port_buffer = unsafe {
            let ptr = NonNull::new_unchecked(
                self.scope
                    .client
                    .lib
                    .jack_port_get_buffer(self.as_ptr().as_ptr(), self.scope.nframes),
            );
            self.scope.client.lib.jack_midi_clear_buffer(ptr.as_ptr());
            ptr
        };

        RefMut::map(refmut, |_| unsafe { std::mem::transmute(port_buffer) })
    }
    #[inline]
    pub fn as_ptr(&self) -> NonNull<sys::jack_port_t> {
        self.port.ptr.port
    }
}

#[doc(alias = "_jack_midi_event")]
#[doc(alias = "jack_midi_event_t")]
#[derive(Debug)]
pub struct MidiEvent<'p> {
    pub time: u32,
    pub data: &'p [u8],
}

#[repr(transparent)]
#[derive(Debug)]
pub struct MidiInput<'p> {
    port_buffer: NonNull<c_void>,
    client: PhantomData<&'p ClientHandle>,
}

impl<'p> MidiInput<'p> {
    #[doc(alias = "jack_midi_get_event_count")]
    pub fn len(&self) -> u32 {
        unsafe { library().jack_midi_get_event_count(self.port_buffer.as_ptr()) }
    }
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    #[inline]
    unsafe fn buffer_get<'e>(
        port_buffer: NonNull<c_void>,
        event_index: u32,
    ) -> std::io::Result<MidiEvent<'e>> {
        let mut event = MaybeUninit::uninit();
        let ret =
            library().jack_midi_event_get(event.as_mut_ptr(), port_buffer.as_ptr(), event_index);
        if ret < 0 {
            return Err(std::io::Error::from_raw_os_error(-ret));
        }
        let event = event.assume_init();
        Ok(MidiEvent {
            time: event.time,
            data: std::slice::from_raw_parts(event.buffer, event.size as usize),
        })
    }
    #[doc(alias = "jack_midi_event_get")]
    pub fn get(&self, event_index: u32) -> std::io::Result<MidiEvent> {
        unsafe { Self::buffer_get(self.port_buffer, event_index) }
    }
    #[doc(alias = "jack_midi_get_lost_event_count")]
    pub fn lost_event_count(&self) -> u32 {
        unsafe { library().jack_midi_get_lost_event_count(self.port_buffer.as_ptr()) }
    }
    pub fn iter(&self) -> MidiInputIter {
        MidiInputIter {
            port_buffer: self.port_buffer,
            index: 0,
            len: self.len(),
            client: PhantomData,
        }
    }
}

impl<'p> IntoIterator for MidiInput<'p> {
    type Item = MidiEvent<'p>;
    type IntoIter = MidiInputIter<'p>;
    fn into_iter(self) -> Self::IntoIter {
        MidiInputIter {
            port_buffer: self.port_buffer,
            index: 0,
            len: self.len(),
            client: PhantomData,
        }
    }
}

#[derive(Debug)]
pub struct MidiInputIter<'p> {
    port_buffer: NonNull<c_void>,
    index: u32,
    len: u32,
    client: PhantomData<&'p ClientHandle>,
}

impl<'p> Iterator for MidiInputIter<'p> {
    type Item = MidiEvent<'p>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.index >= self.len {
            return None;
        }
        let event = unsafe { MidiInput::buffer_get(self.port_buffer, self.index) }.ok()?;
        self.index += 1;
        Some(event)
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        let hint = (self.len - self.index) as usize;
        (hint, Some(hint))
    }
}

impl<'p> std::iter::DoubleEndedIterator for MidiInputIter<'p> {
    fn next_back(&mut self) -> Option<Self::Item> {
        if self.len == 0 {
            return None;
        }
        let event = unsafe { MidiInput::buffer_get(self.port_buffer, self.len - 1) }.ok()?;
        self.len -= 1;
        Some(event)
    }
}

impl<'p> std::iter::FusedIterator for MidiInputIter<'p> {}
impl<'p> std::iter::ExactSizeIterator for MidiInputIter<'p> {}

#[repr(transparent)]
#[derive(Debug)]
pub struct MidiWriter<'p> {
    port_buffer: NonNull<c_void>,
    client: PhantomData<&'p ClientHandle>,
}

impl<'p> MidiWriter<'p> {
    #[doc(alias = "jack_midi_max_event_size")]
    pub fn max_event_size(&self) -> usize {
        unsafe { library().jack_midi_max_event_size(self.port_buffer.as_ptr()) as usize }
    }
    #[doc(alias = "jack_midi_get_lost_event_count")]
    pub fn lost_event_count(&self) -> u32 {
        unsafe { library().jack_midi_get_lost_event_count(self.port_buffer.as_ptr()) }
    }
    #[doc(alias = "jack_midi_event_write")]
    pub fn write(&self, time: u32, event: &MidiEvent) -> std::io::Result<()> {
        let ret = unsafe {
            library().jack_midi_event_write(
                self.port_buffer.as_ptr(),
                time,
                event.data.as_ptr(),
                event.data.len() as _,
            )
        };
        if ret < 0 {
            return Err(std::io::Error::from_raw_os_error(-ret));
        }
        Ok(())
    }
}

#[derive(Debug)]
pub struct CycleTimes {
    pub current_frames: Frames,
    pub current_time: Time,
    pub next_time: Time,
    pub period: Duration,
}

#[derive(Debug)]
pub(crate) struct ProcessPortOuter<PortData>(pub Rc<ProcessPortInner<PortData>>);

impl<PortData> ProcessPortOuter<PortData> {
    pub fn new(ptr: PortPtr, data: PortData) -> Self {
        Self(Rc::new(ProcessPortInner {
            ptr,
            data: RefCell::new(data),
            buffer_lock: Default::default(),
        }))
    }
}

impl<PortData> Clone for ProcessPortOuter<PortData> {
    #[inline]
    fn clone(&self) -> Self {
        debug_assert_eq!(std::thread::current().id(), self.0.ptr.thread);
        Self(self.0.clone())
    }
}

// SAFETY:
// only drop or clone this on the main thread
unsafe impl<PortData> Send for ProcessPortOuter<PortData> {}

#[derive(Debug)]
pub(crate) struct ProcessPortInner<PortData> {
    pub ptr: PortPtr,
    data: RefCell<PortData>,
    buffer_lock: RefCell<()>,
}

#[derive(Debug)]
pub(crate) struct PortPtr {
    #[cfg(debug_assertions)]
    pub(crate) thread: std::thread::ThreadId,
    pub(crate) client: ClientHandle,
    pub(crate) port: NonNull<sys::jack_port_t>,
    pub(crate) flags: PortFlags,
    pub(crate) port_type: PortType,
}

impl PortPtr {
    pub unsafe fn new(
        client: ClientHandle,
        port: *mut sys::jack_port_t,
        flags: PortFlags,
        port_type: PortType,
    ) -> Option<Self> {
        Some(Self {
            #[cfg(debug_assertions)]
            thread: std::thread::current().id(),
            client,
            port: NonNull::new(port)?,
            flags,
            port_type,
        })
    }
}

impl Drop for PortPtr {
    fn drop(&mut self) {
        debug_assert_eq!(std::thread::current().id(), self.thread);
        unsafe {
            self.client
                .lib
                .jack_port_unregister(self.client.as_ptr(), self.port.as_ptr());
        }
    }
}

#[derive(Debug)]
pub(crate) struct ProcessPorts<PortData> {
    pub ports: HashMap<OwnedPortUuid, ProcessPortOuter<PortData>>,
}

impl<PortData> Default for ProcessPorts<PortData> {
    fn default() -> Self {
        Self {
            ports: HashMap::new(),
        }
    }
}
