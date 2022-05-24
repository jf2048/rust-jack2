use crate::{sys, PropertyKey};

#[doc(alias = "JACK_METADATA_CONNECTED")]
pub fn connected() -> PropertyKey {
    PropertyKey(unsafe { sys::JACK_METADATA_CONNECTED })
}
#[doc(alias = "JACK_METADATA_EVENT_TYPES")]
pub fn event_types() -> PropertyKey {
    PropertyKey(unsafe { sys::JACK_METADATA_EVENT_TYPES })
}
#[doc(alias = "JACK_METADATA_HARDWARE")]
pub fn hardware() -> PropertyKey {
    PropertyKey(unsafe { sys::JACK_METADATA_HARDWARE })
}
#[doc(alias = "JACK_METADATA_ICON_LARGE")]
pub fn icon_large() -> PropertyKey {
    PropertyKey(unsafe { sys::JACK_METADATA_ICON_LARGE })
}
#[doc(alias = "JACK_METADATA_ICON_NAME")]
pub fn icon_name() -> PropertyKey {
    PropertyKey(unsafe { sys::JACK_METADATA_ICON_NAME })
}
#[doc(alias = "JACK_METADATA_ICON_SMALL")]
pub fn icon_small() -> PropertyKey {
    PropertyKey(unsafe { sys::JACK_METADATA_ICON_SMALL })
}
#[doc(alias = "JACK_METADATA_ORDER")]
pub fn order() -> PropertyKey {
    PropertyKey(unsafe { sys::JACK_METADATA_ORDER })
}
#[doc(alias = "JACK_METADATA_PRETTY_NAME")]
pub fn pretty_name() -> PropertyKey {
    PropertyKey(unsafe { sys::JACK_METADATA_PRETTY_NAME })
}
#[doc(alias = "JACK_METADATA_PORT_GROUP")]
pub fn port_group() -> PropertyKey {
    PropertyKey(unsafe { sys::JACK_METADATA_PORT_GROUP })
}
#[doc(alias = "JACK_METADATA_SIGNAL_TYPE")]
pub fn signal_type() -> PropertyKey {
    PropertyKey(unsafe { sys::JACK_METADATA_SIGNAL_TYPE })
}
