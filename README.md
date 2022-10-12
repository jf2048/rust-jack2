# jack2

A safe Rust binding for [JACK Audio Connection Kit](https://jackaudio.org) with some advanced
features.

The goal of this binding is to have 100% safe API coverage for the subset of all API features
supported by both [`jackd2`](https://github.com/jackaudio/jack2) and by
[`pipewire-jack`](https://gitlab.freedesktop.org/pipewire/pipewire/-/wikis/Config-JACK). These
bindings will not support obscure or deprecated libjack features, such as internal clients,
session management, netjack, client threads, or jackctl. Features present only in
[`jackd1`](https://github.com/jackaudio/jack1) will not be supported. The JACK ringbuffer
is also not supported; use one of the many native Rust ring buffer or bounded channel
implementations instead.

This crate requires an async runtime to work correctly. Support for the
[`tokio`](https://tokio.rs/) and gtk-rs [`glib`](https://gtk-rs.org/) runtimes
are included as feature flags, but additional runtimes can be supported via a
trait.

## Example

```rust
#[tokio::main(flavor = "current_thread")]
async fn main() {
    let (client, _) = jack2::ClientBuilder::<jack2::tokio::TokioContext>::new("sine")
        .flags(jack2::ClientOptions::NO_START_SERVER)
        .build()
        .unwrap();

    // create a single output port
    let port = client
        .register_ports([jack2::PortInfo {
            name: "Mono Out",
            type_: jack2::PortType::Audio,
            mode: jack2::PortMode::Output,
            flags: jack2::PortCreateFlags::empty(),
            data: (),
        }])
        .remove(0)
        .unwrap();

    // render a simple 440 Hz sine wave
    let handler = jack2::ClosureProcessHandler::new(move |scope| {
        const FREQ: f64 = 440.0;
        let freq_r = scope.sample_rate() as f64 / FREQ;
        let time = u32::from(scope.last_frame_time());
        let port = scope.port_by_owned_uuid(port).unwrap();
        let mut buf = port.audio_out_buffer();
        for (i, sample) in buf.iter_mut().enumerate() {
            *sample = (f64::from(time.wrapping_add(i as u32)) / freq_r
                * (std::f64::consts::PI * 2.0))
                .sin() as f32;
        }
        std::ops::ControlFlow::Continue(())
    });

    // keep the client around until the user presses Ctrl+C
    let _client = client
        .activate(Some(handler), None::<()>, jack2::ActivateFlags::empty())
        .unwrap();
    tokio::signal::ctrl_c().await.unwrap();
}
```

## See Also

The [`jack`](https://docs.rs/jack/) crate was written several years before the `jack2` crate,
by a different author. The `jack2` crate is not based on it, but it does take inspiration from
some concepts when it makes sense, and some type names are intentionally kept similar for ease
of migration. This crate could be compared to a complete rewrite of `jack`,
with a cleaned-up API.
