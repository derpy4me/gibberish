use gibberish_client::{DesktopIpcTransport, SlintController, UiEvent};
use std::error::Error;

fn main() -> Result<(), Box<dyn Error>> {
    env_logger::init_from_env(env_logger::Env::default().default_filter_or("info"));

    // Tokio multithread runtime for background network & IPC transport
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    let controller = SlintController::new()?;
    let sender = controller.event_sender();

    // Start background IPC transport
    let transport = DesktopIpcTransport::new("ws://127.0.0.1:4483", sender.clone());
    controller.set_transport(transport.clone());

    let _enter = rt.enter();
    transport.start();

    // Populate initial telemetry
    let _ = sender.send(UiEvent::TelemetryUpdated {
        tx: 0,
        rx: 0,
        channel: 15,
        avg_lqi: 0,
        status: "CONNECTING".to_string(),
    });

    controller.run()?;
    Ok(())
}
