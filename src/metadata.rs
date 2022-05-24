use crate::{sys, PropertyKey};

pub fn connected() -> PropertyKey {
    PropertyKey(unsafe { sys::JACK_METADATA_CONNECTED })
}
pub fn event_types() -> PropertyKey {
    PropertyKey(unsafe { sys::JACK_METADATA_EVENT_TYPES })
}
pub fn hardware() -> PropertyKey {
    PropertyKey(unsafe { sys::JACK_METADATA_HARDWARE })
}
pub fn icon_large() -> PropertyKey {
    PropertyKey(unsafe { sys::JACK_METADATA_ICON_LARGE })
}
pub fn icon_name() -> PropertyKey {
    PropertyKey(unsafe { sys::JACK_METADATA_ICON_NAME })
}
pub fn icon_small() -> PropertyKey {
    PropertyKey(unsafe { sys::JACK_METADATA_ICON_SMALL })
}
pub fn order() -> PropertyKey {
    PropertyKey(unsafe { sys::JACK_METADATA_ORDER })
}
pub fn pretty_name() -> PropertyKey {
    PropertyKey(unsafe { sys::JACK_METADATA_PRETTY_NAME })
}
pub fn port_group() -> PropertyKey {
    PropertyKey(unsafe { sys::JACK_METADATA_PORT_GROUP })
}
pub fn signal_type() -> PropertyKey {
    PropertyKey(unsafe { sys::JACK_METADATA_SIGNAL_TYPE })
}
