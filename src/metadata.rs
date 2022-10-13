//! Built-in metadata property keys.
//!
//! See [JACK Metadata API](https://jackaudio.org/metadata/) for a description of how to use
//! metadata values.
//!
//! Reading metadata is done with [`crate::property`], [`fn@crate::properties`], and
//! [`crate::all_properties`].
//!
//! Modifying metadata is done with [`Client::set_property`](crate::Client::set_property).
//!
//! Deleting metadata is done with [`Client::remove_property`](crate::Client::remove_property),
//! [`Client::remove_properties`](crate::Client::remove_properties), and
//! [`Client::remove_all_properties`](crate::Client::remove_all_properties).

use crate::{sys, PropertyKey};

/// A value that identifies what the hardware port is connected to (an external device of some
/// kind).
///
/// Possible values might be "E-Piano" or "Master 2 Track".
#[doc(alias = "JACK_METADATA_CONNECTED")]
pub fn connected() -> PropertyKey {
    PropertyKey(unsafe { sys::JACK_METADATA_CONNECTED })
}

/// The supported event types of an event port.
///
/// This is a kludge around JACK only supporting MIDI, particularly for OSC. This property is a
/// comma-separated list of event types, currently "MIDI" or "OSC".  If this contains "OSC", the
/// port may carry OSC bundles (first byte '#') or OSC messages (first byte '/').  Note that the
/// "status byte" of both OSC events is not a valid MIDI status byte, so MIDI clients that check
/// the status byte will gracefully ignore OSC messages if the user makes an inappropriate
/// connection.
#[doc(alias = "JACK_METADATA_EVENT_TYPES")]
pub fn event_types() -> PropertyKey {
    PropertyKey(unsafe { sys::JACK_METADATA_EVENT_TYPES })
}

/// A value that should be shown when attempting to identify the specific hardware outputs of a
/// client.
///
/// Typical values might be "ADAT1", "S/PDIF L" or "MADI 43".
#[doc(alias = "JACK_METADATA_HARDWARE")]
pub fn hardware() -> PropertyKey {
    PropertyKey(unsafe { sys::JACK_METADATA_HARDWARE })
}

/// A binary value for a large PNG icon.
///
/// This is a value with a MIME type of "image/png;base64" that is an encoding of an N×N (with 32 <
/// N ≤ 128) image to be used when displaying a visual representation of that client or port.
#[doc(alias = "JACK_METADATA_ICON_LARGE")]
pub fn icon_large() -> PropertyKey {
    PropertyKey(unsafe { sys::JACK_METADATA_ICON_LARGE })
}

/// The name of the icon for the subject (typically client).
///
/// This is used for looking up icons on the system, possibly with many sizes or themes.  Icons
/// should be searched for according to the freedesktop Icon
///
/// See also: [Theme Specification](https://specifications.freedesktop.org/icon-theme-spec/icon-theme-spec-latest.html).
#[doc(alias = "JACK_METADATA_ICON_NAME")]
pub fn icon_name() -> PropertyKey {
    PropertyKey(unsafe { sys::JACK_METADATA_ICON_NAME })
}

/// A binary value for a small PNG icon.
///
/// This is a value with a MIME type of "image/png;base64" that is an encoding of an N×N (with N
/// ≤ 32) image to be used when displaying a visual representation of that client or port.
#[doc(alias = "JACK_METADATA_ICON_SMALL")]
pub fn icon_small() -> PropertyKey {
    PropertyKey(unsafe { sys::JACK_METADATA_ICON_SMALL })
}

/// Order for a port.
///
/// This is used to specify the best order to show ports in user interfaces. The value *must* be an
/// integer. There are no other requirements, so there may be gaps in the orders for several ports.
/// Applications should compare the orders of ports to determine their relative order, but must not
/// assign any other relevance to order values.
///
/// It is encouraged to use [`http://www.w3.org/2001/XMLSchema#int`] as the type.
#[doc(alias = "JACK_METADATA_ORDER")]
pub fn order() -> PropertyKey {
    PropertyKey(unsafe { sys::JACK_METADATA_ORDER })
}

/// A value that should be shown to the user when displaying a port to the user.
///
/// Displays unless the user has explicitly overridden that a request to show the port name, or
/// some other key value.
#[doc(alias = "JACK_METADATA_PRETTY_NAME")]
pub fn pretty_name() -> PropertyKey {
    PropertyKey(unsafe { sys::JACK_METADATA_PRETTY_NAME })
}

/// Group for the port.
///
/// This property allows ports to be grouped together logically by a client.
#[doc(alias = "JACK_METADATA_PORT_GROUP")]
pub fn port_group() -> PropertyKey {
    PropertyKey(unsafe { sys::JACK_METADATA_PORT_GROUP })
}

/// The type of an audio signal.
///
/// This property allows audio ports to be tagged with a "meaning". The value is a simple string.
/// Currently, the only type is "CV", for "control voltage" ports. Hosts *should* be take care to
/// not treat CV ports as audibile and send their output directly to speakers. In particular, CV
/// ports are not necessarily periodic at all and may have very high DC.
#[doc(alias = "JACK_METADATA_SIGNAL_TYPE")]
pub fn signal_type() -> PropertyKey {
    PropertyKey(unsafe { sys::JACK_METADATA_SIGNAL_TYPE })
}
