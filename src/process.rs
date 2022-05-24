use std::{
    cell::{RefCell, RefMut},
    collections::HashMap,
    ffi::c_void,
    marker::PhantomData,
    mem::MaybeUninit,
    ptr::NonNull,
    rc::Rc,
    time::Duration,
};

use crate::{
    sys::{self, library},
    ClientHandle, Error, Frames, OwnedPort, PortFlags, PortType, Time, Transport,
};

#[derive(Debug)]
pub struct ProcessScope<'a, PortData> {
    pub(crate) client: &'a ClientHandle,
    pub(crate) ports: &'a mut ProcessPorts<PortData>,
    pub(crate) nframes: u32,
}

impl<'a, PortData> ProcessScope<'a, PortData> {
    #[inline]
    pub fn nframes(&self) -> u32 {
        self.nframes
    }
    #[inline]
    pub fn last_frame_time(&self) -> Frames {
        Frames(unsafe { self.client.lib.jack_last_frame_time(self.client.as_ptr()) })
    }
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
    pub fn port_type_id(&self, port: OwnedPort) -> Option<PortType> {
        self.ports.ports.get(&port).map(|p| p.0.ptr.port_type)
    }
    pub fn port_flags(&self, port: OwnedPort) -> Option<PortFlags> {
        self.ports.ports.get(&port).map(|p| p.0.ptr.flags)
    }
    pub fn port_data(&self, port: OwnedPort) -> Option<&RefCell<PortData>> {
        self.ports.ports.get(&port).map(|p| &p.0.data)
    }
    pub fn port_audio_in_buffer(&self, port: OwnedPort) -> Option<&[f32]> {
        let port = self.ports.ports.get(&port)?;
        if !port.0.ptr.flags.contains(PortFlags::IS_INPUT)
            || port.0.ptr.port_type != PortType::Audio
        {
            return None;
        }
        unsafe {
            let buf = self
                .client
                .lib
                .jack_port_get_buffer(port.0.ptr.port.as_ptr(), self.nframes);
            Some(std::slice::from_raw_parts(
                buf as *const f32,
                self.nframes as usize,
            ))
        }
    }
    pub fn port_audio_out_buffer(&self, port: OwnedPort) -> Option<RefMut<[f32]>> {
        let port = self.ports.ports.get(&port)?;
        if !port.0.ptr.flags.contains(PortFlags::IS_OUTPUT)
            || port.0.ptr.port_type != PortType::Audio
        {
            return None;
        }
        let refmut = port.0.buffer_lock.try_borrow_mut().ok()?;
        let slice = unsafe {
            let buf = self
                .client
                .lib
                .jack_port_get_buffer(port.0.ptr.port.as_ptr(), self.nframes);
            std::slice::from_raw_parts_mut(buf as *mut f32, self.nframes as usize)
        };
        Some(RefMut::map(refmut, |_| slice))
    }
    pub fn port_midi_in_buffer(&self, port: OwnedPort) -> Option<MidiInput> {
        let port = self.ports.ports.get(&port)?;
        if !port.0.ptr.flags.contains(PortFlags::IS_INPUT) || port.0.ptr.port_type != PortType::Midi
        {
            return None;
        }
        let port_buffer = unsafe {
            NonNull::new_unchecked(
                self.client
                    .lib
                    .jack_port_get_buffer(port.0.ptr.port.as_ptr(), self.nframes),
            )
        };
        Some(MidiInput {
            port_buffer,
            client: PhantomData,
        })
    }
    pub fn port_midi_out_buffer(&self, port: OwnedPort) -> Option<RefMut<MidiWriter>> {
        let port = self.ports.ports.get(&port)?;
        if !port.0.ptr.flags.contains(PortFlags::IS_OUTPUT)
            || port.0.ptr.port_type != PortType::Midi
        {
            return None;
        }
        let refmut = port.0.buffer_lock.try_borrow_mut().ok()?;
        let port_buffer = unsafe {
            let ptr = NonNull::new_unchecked(
                self.client
                    .lib
                    .jack_port_get_buffer(port.0.ptr.port.as_ptr(), self.nframes),
            );
            self.client.lib.jack_midi_clear_buffer(ptr.as_ptr());
            ptr
        };

        Some(RefMut::map(refmut, |_| unsafe {
            std::mem::transmute(port_buffer)
        }))
    }
}

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
    pub fn get(&self, event_index: u32) -> std::io::Result<MidiEvent> {
        unsafe { Self::buffer_get(self.port_buffer, event_index) }
    }
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
    pub fn max_event_size(&self) -> usize {
        unsafe { library().jack_midi_max_event_size(self.port_buffer.as_ptr()) as usize }
    }
    pub fn lost_event_count(&self) -> u32 {
        unsafe { library().jack_midi_get_lost_event_count(self.port_buffer.as_ptr()) }
    }
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
pub(crate) struct ProcessPort<PortData>(pub Rc<ProcessPortInner<PortData>>);

impl<PortData> ProcessPort<PortData> {
    pub fn new(ptr: PortPtr, data: PortData) -> Self {
        Self(Rc::new(ProcessPortInner {
            ptr,
            data: RefCell::new(data),
            buffer_lock: Default::default(),
        }))
    }
}

impl<PortData> Clone for ProcessPort<PortData> {
    #[inline]
    fn clone(&self) -> Self {
        debug_assert_eq!(std::thread::current().id(), self.0.ptr.thread);
        Self(self.0.clone())
    }
}

// SAFETY:
// only drop or clone this on the main thread
unsafe impl<PortData> Send for ProcessPort<PortData> {}

#[derive(Debug)]
pub(crate) struct ProcessPortInner<PortData> {
    pub ptr: PortPtr,
    data: RefCell<PortData>,
    buffer_lock: RefCell<()>,
}

#[derive(Debug)]
pub(crate) struct PortPtr {
    #[cfg(debug_assertions)]
    pub thread: std::thread::ThreadId,
    pub client: NonNull<ClientHandle>,
    pub port: NonNull<sys::jack_port_t>,
    pub flags: PortFlags,
    pub port_type: PortType,
}

impl PortPtr {
    pub unsafe fn new(
        client: &ClientHandle,
        port: *mut sys::jack_port_t,
        flags: PortFlags,
        port_type: PortType,
    ) -> Option<Self> {
        Some(Self {
            #[cfg(debug_assertions)]
            thread: std::thread::current().id(),
            client: NonNull::new_unchecked(client as *const _ as *mut _),
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
            let client = self.client.as_ref();
            client
                .lib
                .jack_port_unregister(client.client.as_ptr(), self.port.as_ptr());
        }
    }
}

#[derive(Debug)]
pub(crate) struct ProcessPorts<PortData> {
    pub ports: HashMap<OwnedPort, ProcessPort<PortData>>,
}

impl<PortData> Default for ProcessPorts<PortData> {
    fn default() -> Self {
        Self {
            ports: HashMap::new(),
        }
    }
}
