#![allow(non_upper_case_globals)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
#![allow(unaligned_references)]
#![allow(deref_nullptr)]
#![allow(clippy::unused_unit)]

include!(concat!(env!("OUT_DIR"), "/jack-bindings.rs"));

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

pub unsafe fn library() -> &'static Jack {
    use once_cell::sync::OnceCell as SyncOnceCell;
    static LIBRARY: SyncOnceCell<&'static Jack> = SyncOnceCell::new();
    LIBRARY.get_or_init(|| weak_library().unwrap())
}
