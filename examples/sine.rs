#[derive(clap::Parser)]
struct Args {
    #[clap(long, short, default_value_t = 440.0)]
    frequency: f64,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = <Args as clap::Parser>::parse();
    let (client, _) = jack2::ClientBuilder::<jack2::tokio::TokioContext>::new("rust-jack2-sine")
        .flags(jack2::ClientOptions::NO_START_SERVER)
        .build()?;

    let port = client
        .register_ports([jack2::PortInfo {
            name: "Mono Out",
            type_: jack2::PortType::Audio,
            mode: jack2::PortMode::Output,
            flags: jack2::PortCreateFlags::empty(),
            data: (),
        }])
        .remove(0)?;

    let handler = jack2::ClosureProcessHandler::new(move |scope| {
        let freq_r = scope.sample_rate() as f64 / args.frequency;
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

    let _client = client.activate(Some(handler), None::<()>, jack2::ActivateFlags::empty())?;
    tokio::signal::ctrl_c().await?;
    Ok(())
}
