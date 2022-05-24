use std::mem::MaybeUninit;

use crate::{sys, ClientHandle, Error, Frames, Time};

pub struct Transport<'c> {
    client: &'c ClientHandle,
}

impl<'c> Transport<'c> {
    #[inline]
    pub(crate) fn new(client: &'c ClientHandle) -> Self {
        Self { client }
    }
    #[doc(alias = "jack_transport_locate")]
    pub fn locate(&self, frame: Frames) -> crate::Result<()> {
        Error::check_ret(unsafe {
            self.client
                .lib
                .jack_transport_locate(self.client.as_ptr(), frame.into())
        })
    }
    #[doc(alias = "jack_transport_query")]
    pub fn query(&self) -> (TransportState, TransportPosition) {
        let mut pos = MaybeUninit::<sys::jack_position_t>::uninit();
        let (state, pos) = unsafe {
            let state = self
                .client
                .lib
                .jack_transport_query(self.client.as_ptr(), pos.as_mut_ptr());
            (state, pos.assume_init())
        };
        (TransportState::from_jack(state), TransportPosition(pos))
    }
    #[doc(alias = "jack_get_current_transport_frame")]
    pub fn current_transport_frame(&self) -> Frames {
        (unsafe {
            self.client
                .lib
                .jack_get_current_transport_frame(self.client.as_ptr())
        })
        .into()
    }
    #[doc(alias = "jack_transport_reposition")]
    pub fn reposition(&self, pos: &TransportPosition) -> crate::Result<()> {
        if (pos.0.valid
            & !(sys::jack_position_bits_t_JackPositionBBT
                | sys::jack_position_bits_t_JackPositionTimecode))
            != 0
        {
            return Err(Error::invalid_option());
        }
        Error::check_ret(unsafe {
            self.client
                .lib
                .jack_transport_reposition(self.client.as_ptr(), &pos.0)
        })
    }
    #[doc(alias = "jack_transport_start")]
    pub fn start(&self) {
        unsafe {
            self.client.lib.jack_transport_start(self.client.as_ptr());
        }
    }
    #[doc(alias = "jack_transport_stop")]
    pub fn stop(&self) {
        unsafe {
            self.client.lib.jack_transport_stop(self.client.as_ptr());
        }
    }
}

#[doc(alias = "jack_transport_state_t")]
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub enum TransportState {
    #[doc(alias = "JackTransportStopped")]
    Stopped,
    #[doc(alias = "JackTransportRolling")]
    Rolling,
    #[doc(alias = "JackTransportStarting")]
    Starting,
    #[doc(alias = "JackTransportNetStarting")]
    NetStarting,
}

impl TransportState {
    pub(crate) fn from_jack(state: sys::jack_transport_state_t) -> Self {
        match state {
            sys::jack_transport_state_t_JackTransportStopped => Self::Stopped,
            sys::jack_transport_state_t_JackTransportRolling => Self::Rolling,
            sys::jack_transport_state_t_JackTransportStarting => Self::Starting,
            sys::jack_transport_state_t_JackTransportNetStarting => Self::NetStarting,
            _ => Self::Stopped,
        }
    }
}

#[doc(alias = "jack_position_t")]
#[repr(transparent)]
#[derive(Debug)]
pub struct TransportPosition(sys::jack_position_t);

impl TransportPosition {
    #[inline]
    pub fn usecs(&self) -> Time {
        Time(self.0.usecs)
    }
    #[inline]
    pub fn frame_rate(&self) -> u32 {
        self.0.frame_rate
    }
    #[inline]
    pub fn frame(&self) -> u32 {
        self.0.frame
    }
    #[doc(alias = "JackPositionBBT")]
    #[inline]
    pub fn has_bbt(&self) -> bool {
        (self.0.valid & sys::jack_position_bits_t_JackPositionBBT) != 0
    }
    #[inline]
    pub fn bar(&self) -> i32 {
        self.0.bar
    }
    #[inline]
    pub fn beat(&self) -> i32 {
        self.0.beat
    }
    #[inline]
    pub fn tick(&self) -> i32 {
        self.0.tick
    }
    #[inline]
    pub fn bar_start_tick(&self) -> f64 {
        self.0.bar_start_tick
    }
    #[inline]
    pub fn beats_per_bar(&self) -> f32 {
        self.0.beats_per_bar
    }
    #[inline]
    pub fn beat_type(&self) -> f32 {
        self.0.beat_type
    }
    #[inline]
    pub fn ticks_per_beat(&self) -> f64 {
        self.0.ticks_per_beat
    }
    #[inline]
    pub fn beats_per_minute(&self) -> f64 {
        self.0.beats_per_minute
    }
    #[doc(alias = "JackPositionTimecode")]
    #[inline]
    pub fn has_timecode(&self) -> bool {
        (self.0.valid & sys::jack_position_bits_t_JackPositionTimecode) != 0
    }
    #[inline]
    pub fn frame_time(&self) -> f64 {
        self.0.frame_time
    }
    #[inline]
    pub fn next_time(&self) -> f64 {
        self.0.next_time
    }
    #[doc(alias = "JackBBTFrameOffset")]
    #[inline]
    pub fn has_bbt_offset(&self) -> bool {
        (self.0.valid & sys::jack_position_bits_t_JackBBTFrameOffset) != 0
    }
    #[inline]
    pub fn bbt_offset(&self) -> u32 {
        self.0.bbt_offset
    }
    #[doc(alias = "JackAudioVideoRatio")]
    #[inline]
    pub fn has_audio_video_ratio(&self) -> bool {
        (self.0.valid & sys::jack_position_bits_t_JackAudioVideoRatio) != 0
    }
    #[inline]
    pub fn audio_video_ratio(&self) -> f32 {
        self.0.audio_frames_per_video_frame
    }
    #[doc(alias = "JackVideoFrameOffset")]
    #[inline]
    pub fn has_video_offset(&self) -> bool {
        (self.0.valid & sys::jack_position_bits_t_JackVideoFrameOffset) != 0
    }
    #[inline]
    pub fn video_offset(&self) -> u32 {
        self.0.video_offset
    }
    #[cfg(feature = "v1_9_19")]
    #[doc(alias = "JackTickDouble")]
    #[inline]
    pub fn has_tick_double(&self) -> bool {
        (self.0.valid & sys::jack_position_bits_t_JackTickDouble) != 0
    }
    #[cfg(feature = "v1_9_19")]
    #[inline]
    pub fn tick_double(&self) -> f64 {
        self.0.tick_double
    }
    #[inline]
    pub fn set_bbt(&mut self, bbt: TransportBBT) {
        self.0.valid |= sys::jack_position_bits_t_JackPositionBBT;
        self.0.bar = bbt.bar;
        self.0.beat = bbt.beat;
        self.0.tick = bbt.tick;
        self.0.bar_start_tick = bbt.bar_start_tick;
        self.0.beats_per_bar = bbt.beats_per_bar;
        self.0.beat_type = bbt.beat_type;
        self.0.ticks_per_beat = bbt.ticks_per_beat;
        self.0.beats_per_minute = bbt.beats_per_minute;
    }
    #[inline]
    pub fn unset_bbt(&mut self) {
        self.0.valid &= !sys::jack_position_bits_t_JackPositionBBT;
    }
    #[inline]
    pub fn set_timecode(&mut self, timecode: TransportTimecode) {
        self.0.valid |= sys::jack_position_bits_t_JackPositionTimecode;
        self.0.frame_time = timecode.frame_time;
        self.0.next_time = timecode.next_time;
    }
    #[inline]
    pub fn unset_timecode(&mut self) {
        self.0.valid &= !sys::jack_position_bits_t_JackPositionTimecode;
    }
    #[inline]
    pub fn set_bbt_offset(&mut self, bbt_offset: u32) {
        self.0.valid |= sys::jack_position_bits_t_JackBBTFrameOffset;
        self.0.bbt_offset = bbt_offset;
    }
    #[inline]
    pub fn unset_bbt_offset(&mut self) {
        self.0.valid &= !sys::jack_position_bits_t_JackBBTFrameOffset;
    }
    #[inline]
    pub fn set_audio_video_ratio(&mut self, audio_video_ratio: f32) {
        self.0.valid |= sys::jack_position_bits_t_JackAudioVideoRatio;
        self.0.audio_frames_per_video_frame = audio_video_ratio;
    }
    #[inline]
    pub fn unset_audio_video_ratio(&mut self) {
        self.0.valid &= !sys::jack_position_bits_t_JackAudioVideoRatio;
    }
    #[inline]
    pub fn set_video_offset(&mut self, video_offset: u32) {
        self.0.valid |= sys::jack_position_bits_t_JackVideoFrameOffset;
        self.0.video_offset = video_offset;
    }
    #[inline]
    pub fn unset_video_offset(&mut self) {
        self.0.valid &= !sys::jack_position_bits_t_JackVideoFrameOffset;
    }
    #[cfg(feature = "v1_9_19")]
    #[inline]
    pub fn set_tick_double(&mut self, tick_double: f64) {
        self.0.valid |= sys::jack_position_bits_t_JackTickDouble;
        self.0.tick_double = tick_double;
    }
    #[cfg(feature = "v1_9_19")]
    #[inline]
    pub fn unset_tick_double(&mut self) {
        self.0.valid &= !sys::jack_position_bits_t_JackTickDouble;
    }
}

impl Default for TransportPosition {
    fn default() -> Self {
        Self(unsafe { MaybeUninit::zeroed().assume_init() })
    }
}

#[derive(Debug, Copy, Clone)]
pub struct TransportBBT {
    pub bar: i32,
    pub beat: i32,
    pub tick: i32,
    pub bar_start_tick: f64,
    pub beats_per_bar: f32,
    pub beat_type: f32,
    pub ticks_per_beat: f64,
    pub beats_per_minute: f64,
}

#[derive(Debug, Copy, Clone)]
pub struct TransportTimecode {
    pub frame_time: f64,
    pub next_time: f64,
}
