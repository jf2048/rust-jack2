//! C FFI bindings to libjack shared library.

#![allow(non_upper_case_globals)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
#![allow(unaligned_references)]
#![allow(deref_nullptr)]
#![allow(clippy::unused_unit)]
#![allow(clippy::missing_safety_doc)]
#![allow(missing_docs)]
#![allow(rustdoc::bare_urls)]

include!(concat!(env!("OUT_DIR"), "/jack-bindings.rs"));

impl Jack {
    /// Returns the inner library object.
    #[inline]
    pub fn inner(&self) -> &libloading::Library {
        &self.__library
    }
}

impl std::fmt::Debug for Jack {
    #[inline]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Jack")
            .field("__library", &self.__library)
            .finish()
    }
}

/// Attempts to load a static reference to the JACK dynamic library.
///
/// Returns [`Err`] if the JACK library could not be loaded. This function caches its return value;
/// all subsequent calls after the first will return the same result.
pub unsafe fn weak_library() -> Result<&'static Jack, &'static libloading::Error> {
    const LIB_NAME: &str = if cfg!(windows) {
        if cfg!(target_arch = "x86") {
            "libjack.dll"
        } else {
            "libjack64.dll"
        }
    } else if cfg!(target_vendor = "apple") {
        "libjack.0.dylib"
    } else {
        "libjack.so.0"
    };

    use once_cell::sync::OnceCell as SyncOnceCell;
    static LIBRARY: SyncOnceCell<Result<Jack, libloading::Error>> = SyncOnceCell::new();
    LIBRARY.get_or_init(|| Jack::new(LIB_NAME)).as_ref()
}

/// Returns a static reference to the JACK dynamic library.
///
/// # Panics
///
/// Panics if the JACK library could not be loaded.
pub unsafe fn library() -> &'static Jack {
    use once_cell::sync::OnceCell as SyncOnceCell;
    static LIBRARY: SyncOnceCell<&'static Jack> = SyncOnceCell::new();
    LIBRARY.get_or_init(|| weak_library().unwrap())
}
