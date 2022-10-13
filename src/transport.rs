use std::mem::MaybeUninit;

use crate::{sys, ClientHandle, Error, Frames, Time};

/// Helper structure to control the [JACK
/// transport](https://jackaudio.org/api/transport-design.html).
///
/// This separate object exists becuse transport functions can be called from both the client main
/// thread and the process thread. Use [`Client::transport`](crate::Client::transport) or
/// [`ProcessScope::transport`](crate::ProcessScope::transport) to get a handle.
pub struct Transport<'c> {
    client: &'c ClientHandle,
}

impl<'c> Transport<'c> {
    #[inline]
    pub(crate) fn new(client: &'c ClientHandle) -> Self {
        Self { client }
    }
    /// Reposition the transport to a new frame number.
    ///
    /// The new position takes effect in two process cycles. If there are slow-sync clients and the
    /// transport is already rolling, it will enter the [`Starting`](TransportState::Starting)
    /// state and begin invoking their [`ProcessHandler::sync`](crate::ProcessHandler::sync) until
    /// ready. This method is real-time-safe.
    ///
    /// `frame` if the frame number of new transport position.
    ///
    /// See also [`Self::reposition`].
    #[doc(alias = "jack_transport_locate")]
    pub fn locate(&self, frame: Frames) -> crate::Result<()> {
        Error::check_ret(unsafe {
            self.client
                .lib
                .jack_transport_locate(self.client.as_ptr(), frame.into())
        })
    }
    /// Query the current transport state and position.
    ///
    /// If called from the process thread, the [`TransportPosition`] corresponds to the first frame
    /// of the current cycle and the state returned is valid for the entire cycle. Call the
    /// `TransportPosition::has_*` methods first to see which fields contain valid data.
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
    /// Return an estimate of the current transport frame.
    ///
    /// Includes any time elapsed since the last transport positional update.
    #[doc(alias = "jack_get_current_transport_frame")]
    pub fn current_transport_frame(&self) -> Frames {
        (unsafe {
            self.client
                .lib
                .jack_get_current_transport_frame(self.client.as_ptr())
        })
        .into()
    }
    /// Request a new transport position.
    ///
    /// The new position takes effect in two process cycles. If there are slow-sync clients and the
    /// transport is already rolling, it will enter the [`Starting`](TransportState::Starting)
    /// state and begin invoking their [`ProcessHandler::sync`](crate::ProcessHandler::sync) until
    /// ready. This method is real-time-safe.
    ///
    /// See also [`Self::locate`].
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
    /// Start the JACK transport rolling.
    ///
    /// Any client can make this request at any time. It takes effect no sooner than the next
    /// process cycle, perhaps later if there are slow-sync clients. This method is real-time-safe.
    #[doc(alias = "jack_transport_start")]
    pub fn start(&self) {
        unsafe {
            self.client.lib.jack_transport_start(self.client.as_ptr());
        }
    }
    /// Stop the JACK transport.
    ///
    /// Any client can make this request at any time. It takes effect on the next process cycle.
    /// This method is real-time-safe.
    #[doc(alias = "jack_transport_stop")]
    pub fn stop(&self) {
        unsafe {
            self.client.lib.jack_transport_stop(self.client.as_ptr());
        }
    }
}

#[doc(alias = "jack_transport_state_t")]
/// State of the current transport.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub enum TransportState {
    /// Transport halted.
    #[doc(alias = "JackTransportStopped")]
    Stopped,
    /// Transport playing.
    #[doc(alias = "JackTransportRolling")]
    Rolling,
    /// Waiting for timebase sync to be ready.
    #[doc(alias = "JackTransportStarting")]
    Starting,
    /// Waiting for timebase sync to be ready on the network.
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

/// Structure holding extended positioning information for the JACK transport.
#[doc(alias = "jack_position_t")]
#[repr(transparent)]
#[derive(Debug)]
pub struct TransportPosition(sys::jack_position_t);

impl TransportPosition {
    /// Returns current monotonic, free-rolling time in microseconds.
    #[inline]
    pub fn usecs(&self) -> Time {
        Time(self.0.usecs)
    }
    /// Returns current frame rate (per second).
    #[inline]
    pub fn frame_rate(&self) -> u32 {
        self.0.frame_rate
    }
    /// Returns current frame number.
    #[inline]
    pub fn frame(&self) -> u32 {
        self.0.frame
    }
    /// Returns `true` if the position contains Bar, Beat, Tick information.
    ///
    /// Corresponds to [`bar`](Self::bar), [`beat`](Self::beat), [`tick`](Self::tick),
    /// [`bar_start_tick`](Self::bar_start_tick), [`beats_per_bar`](Self::beats_per_bar),
    /// [`beat_type`](Self::beat_type), [`ticks_per_beat`](Self::ticks_per_beat),
    /// [`beats_per_minute`](Self::beats_per_minute).
    #[doc(alias = "JackPositionBBT")]
    #[inline]
    pub fn has_bbt(&self) -> bool {
        (self.0.valid & sys::jack_position_bits_t_JackPositionBBT) != 0
    }
    /// Returns current bar.
    #[inline]
    pub fn bar(&self) -> i32 {
        self.0.bar
    }
    /// Returns current beat within bar.
    #[inline]
    pub fn beat(&self) -> i32 {
        self.0.beat
    }
    /// Returns current tick within beat.
    #[inline]
    pub fn tick(&self) -> i32 {
        self.0.tick
    }
    /// Returns the tick offset for the start of the current bar.
    #[inline]
    pub fn bar_start_tick(&self) -> f64 {
        self.0.bar_start_tick
    }
    /// Returns time signature "numerator".
    #[inline]
    pub fn beats_per_bar(&self) -> f32 {
        self.0.beats_per_bar
    }
    /// Returns time signature "denominator".
    #[inline]
    pub fn beat_type(&self) -> f32 {
        self.0.beat_type
    }
    /// Returns the number of ticks per beat.
    #[inline]
    pub fn ticks_per_beat(&self) -> f64 {
        self.0.ticks_per_beat
    }
    /// Returns the number of beats per minute.
    #[inline]
    pub fn beats_per_minute(&self) -> f64 {
        self.0.beats_per_minute
    }
    /// Returns `true` if the position contains the external timecode.
    ///
    /// Corresponds to [`frame_time`](Self::frame_time) and [`next_time`](Self::next_time).
    #[doc(alias = "JackPositionTimecode")]
    #[inline]
    pub fn has_timecode(&self) -> bool {
        (self.0.valid & sys::jack_position_bits_t_JackPositionTimecode) != 0
    }
    /// Returns current time in seconds.
    #[inline]
    pub fn frame_time(&self) -> f64 {
        self.0.frame_time
    }
    /// Returns next sequential frame time (unless repositioned).
    #[inline]
    pub fn next_time(&self) -> f64 {
        self.0.next_time
    }
    /// Returns `true` if the position contains the frame offset of BBT information.
    ///
    /// Corresponds to [`bbt_offset`](Self::bbt_offset).
    #[doc(alias = "JackBBTFrameOffset")]
    #[inline]
    pub fn has_bbt_offset(&self) -> bool {
        (self.0.valid & sys::jack_position_bits_t_JackBBTFrameOffset) != 0
    }
    /// Returns frame offset for the BBT fields.
    ///
    /// The given bar, beat, and tick values actually refer to a time `bbt_offset` frames before
    /// the start of the cycle. Should be assumed to be 0 if
    /// [`has_bbt_offset`](Self::has_bbt_offset) is not set. If it is set and this value is zero,
    /// the BBT time refers to the first frame of this cycle. If the value is positive, the BBT
    /// time refers to a frame that many frames before the start of the cycle.
    #[inline]
    pub fn bbt_offset(&self) -> u32 {
        self.0.bbt_offset
    }
    /// Returns `true` if the position contains the audio frames per video frame.
    ///
    /// Corresponds to [`audio_video_ratio`](Self::audio_video_ratio).
    #[doc(alias = "JackAudioVideoRatio")]
    #[inline]
    pub fn has_audio_video_ratio(&self) -> bool {
        (self.0.valid & sys::jack_position_bits_t_JackAudioVideoRatio) != 0
    }
    /// Number of audio frames per video frame.
    ///
    /// Should be assumed zero if [`has_audio_video_ratio`](Self::has_audio_video_ratio) is not
    /// set. If it is set and the value is zero, no video data exists within the JACK graph.
    #[inline]
    pub fn audio_video_ratio(&self) -> f32 {
        self.0.audio_frames_per_video_frame
    }
    /// Returns `true` if the position contains the frame offset of the first video frame.
    ///
    /// Corresponds to [`video_offset`](Self::video_offset).
    #[doc(alias = "JackVideoFrameOffset")]
    #[inline]
    pub fn has_video_offset(&self) -> bool {
        (self.0.valid & sys::jack_position_bits_t_JackVideoFrameOffset) != 0
    }
    /// Audio frame at which the first video frame in this cycle occurs.
    ///
    /// Should be assumed to be 0 if [`has_video_offset`](Self::has_video_offset) is not set. If it
    /// is set, but the value is zero, there is no video frame within this cycle.
    #[inline]
    pub fn video_offset(&self) -> u32 {
        self.0.video_offset
    }
    /// Returns `true` if the position contains the double-resolution tick.
    ///
    /// Corresponds to [`tick_double`](Self::tick_double).
    #[cfg_attr(docsrs, doc(cfg(feature = "v_1_9_19")))]
    #[cfg(feature = "v1_9_19")]
    #[doc(alias = "JackTickDouble")]
    #[inline]
    pub fn has_tick_double(&self) -> bool {
        (self.0.valid & sys::jack_position_bits_t_JackTickDouble) != 0
    }
    /// Current tick-within-beat in double resolution.
    ///
    /// Should be assumed zero if [`has_tick_double`](Self::has_tick_double) is not set.
    #[cfg_attr(docsrs, doc(cfg(feature = "v_1_9_19")))]
    #[cfg(feature = "v1_9_19")]
    #[inline]
    pub fn tick_double(&self) -> f64 {
        self.0.tick_double
    }
    /// Sets BBT transport information.
    ///
    /// After calling this, [`has_bbt`](Self::has_bbt) will return `true`.
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
    /// Clears BBT transport information, disabling its flag.
    ///
    /// After calling this, [`has_bbt`](Self::has_bbt) will return `false`.
    #[inline]
    pub fn unset_bbt(&mut self) {
        self.0.valid &= !sys::jack_position_bits_t_JackPositionBBT;
    }
    /// Sets timecode transport information.
    ///
    /// After calling this, [`has_timecode`](Self::has_timecode) will return `true`.
    #[inline]
    pub fn set_timecode(&mut self, timecode: TransportTimecode) {
        self.0.valid |= sys::jack_position_bits_t_JackPositionTimecode;
        self.0.frame_time = timecode.frame_time;
        self.0.next_time = timecode.next_time;
    }
    /// Clears timecode transport information, disabling its flag.
    ///
    /// After calling this, [`has_timecode`](Self::has_timecode) will return `false`.
    #[inline]
    pub fn unset_timecode(&mut self) {
        self.0.valid &= !sys::jack_position_bits_t_JackPositionTimecode;
    }
    /// Sets BBT offset transport information.
    ///
    /// After calling this, [`has_bbt_offset`](Self::has_bbt_offset) will return `true`.
    #[inline]
    pub fn set_bbt_offset(&mut self, bbt_offset: u32) {
        self.0.valid |= sys::jack_position_bits_t_JackBBTFrameOffset;
        self.0.bbt_offset = bbt_offset;
    }
    /// Clears BBT offset transport information, disabling its flag.
    ///
    /// After calling this, [`has_bbt_offset`](Self::has_bbt_offset) will return `false`.
    #[inline]
    pub fn unset_bbt_offset(&mut self) {
        self.0.valid &= !sys::jack_position_bits_t_JackBBTFrameOffset;
    }
    /// Sets audio frames per video frame transport information.
    ///
    /// After calling this, [`has_audio_video_ratio`](Self::has_audio_video_ratio) will return
    /// `true`.
    #[inline]
    pub fn set_audio_video_ratio(&mut self, audio_video_ratio: f32) {
        self.0.valid |= sys::jack_position_bits_t_JackAudioVideoRatio;
        self.0.audio_frames_per_video_frame = audio_video_ratio;
    }
    /// Clears audio frames per video frame transport information, disabling its flag.
    ///
    /// After calling this, [`has_audio_video_ratio`](Self::has_audio_video_ratio) will return
    /// `false`.
    #[inline]
    pub fn unset_audio_video_ratio(&mut self) {
        self.0.valid &= !sys::jack_position_bits_t_JackAudioVideoRatio;
    }
    /// Sets video frame offset transport information.
    ///
    /// After calling this, [`has_video_offset`](Self::has_video_offset) will return `true`.
    #[inline]
    pub fn set_video_offset(&mut self, video_offset: u32) {
        self.0.valid |= sys::jack_position_bits_t_JackVideoFrameOffset;
        self.0.video_offset = video_offset;
    }
    /// Clears video frame offset transport information, disabling its flag.
    ///
    /// After calling this, [`has_video_offset`](Self::has_video_offset) will return `false`.
    #[inline]
    pub fn unset_video_offset(&mut self) {
        self.0.valid &= !sys::jack_position_bits_t_JackVideoFrameOffset;
    }
    /// Sets double-resolution tick transport information.
    ///
    /// After calling this, [`has_tick_double`](Self::has_tick_double) will return `true`.
    #[cfg_attr(docsrs, doc(cfg(feature = "v_1_9_19")))]
    #[cfg(feature = "v1_9_19")]
    #[inline]
    pub fn set_tick_double(&mut self, tick_double: f64) {
        self.0.valid |= sys::jack_position_bits_t_JackTickDouble;
        self.0.tick_double = tick_double;
    }
    /// Clears double-resolution tick transport information, disabling its flag.
    ///
    /// After calling this, [`has_tick_double`](Self::has_tick_double) will return `false`.
    #[cfg_attr(docsrs, doc(cfg(feature = "v_1_9_19")))]
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

/// Structure holding field inputs for [`TransportPosition::set_bbt`].
#[derive(Debug, Copy, Clone)]
pub struct TransportBBT {
    /// Current bar.
    pub bar: i32,
    /// Current beat within bar.
    pub beat: i32,
    /// Current tick within beat.
    pub tick: i32,
    /// Tick offset for the start of the current bar.
    pub bar_start_tick: f64,
    /// Time signature "numerator".
    pub beats_per_bar: f32,
    /// Time signature "denominator".
    pub beat_type: f32,
    /// Number of ticks in each beat.
    pub ticks_per_beat: f64,
    /// Number of beats per minute.
    pub beats_per_minute: f64,
}

/// Structure holding field inputs for [`TransportPosition::set_timecode`].
#[derive(Debug, Copy, Clone)]
pub struct TransportTimecode {
    /// Current time in seconds.
    pub frame_time: f64,
    /// Next sequential frame time (unless repositioned).
    pub next_time: f64,
}
