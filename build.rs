use std::{env, path::PathBuf};

fn main() {
    system_deps::Config::new().probe().unwrap();
    let bindings = bindgen::Builder::default()
        .header_contents(
            "jack-wrapper.h",
            "\
#include <jack/control.h>
#include <jack/intclient.h>
#include <jack/jack.h>
#include <jack/metadata.h>
#include <jack/midiport.h>
#include <jack/net.h>
#include <jack/ringbuffer.h>
#include <jack/session.h>
#include <jack/statistics.h>
#include <jack/uuid.h>
            ",
        )
        .allowlist_function("jack_.*")
        .allowlist_type("jack_.*")
        .allowlist_type("Jack.*")
        .allowlist_var("JACK_.*")
        .dynamic_library_name("Jack")
        .generate()
        .expect("Unable to generate bindings");

    let out_path = PathBuf::from(env::var("OUT_DIR").unwrap());
    bindings
        .write_to_file(out_path.join("jack-bindings.rs"))
        .expect("Couldn't write bindings!");
}
