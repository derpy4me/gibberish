use gibberish_client::{ChatMessageItem, MainWindow, StationItem};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use slint_snapshot::SnapshotRuntime;
use std::rc::Rc;

#[test]
fn test_render_all_ui_states_snapshots() -> Result<(), Box<dyn std::error::Error>> {
    let out_dir = std::path::Path::new("../../docs/screenshots");
    std::fs::create_dir_all(out_dir)?;

    let runtime = SnapshotRuntime::new()?;
    let ui = MainWindow::new()?;
    runtime.set_size(ui.window(), (860, 560), 1.0)?;

    // 1. Populate mock station roster
    let stations = vec![
        StationItem {
            node_id: SharedString::from("0xBEBCE5B8"),
            alias: SharedString::from("Station-Alpha"),
            rssi: SharedString::from("-64 dBm"),
            lqi: SharedString::from("210"),
            trust_state: SharedString::from("verified"),
            signal_bars: 4,
            selected: false,
        },
        StationItem {
            node_id: SharedString::from("0xCAFE1234"),
            alias: SharedString::from("Field-Recon-02"),
            rssi: SharedString::from("-78 dBm"),
            lqi: SharedString::from("165"),
            trust_state: SharedString::from("unverified"),
            signal_bars: 3,
            selected: false,
        },
        StationItem {
            node_id: SharedString::from("0xDEADBEEF"),
            alias: SharedString::from("HQ-Base-Relay"),
            rssi: SharedString::from("-89 dBm"),
            lqi: SharedString::from("110"),
            trust_state: SharedString::from("unverified"),
            signal_bars: 2,
            selected: false,
        },
    ];
    let stations_model: ModelRc<StationItem> = Rc::new(VecModel::from(stations)).into();
    ui.set_stations(stations_model.clone());

    // 2. Populate mock swarm messages
    let swarm_messages = vec![
        ChatMessageItem {
            id: SharedString::from("m1"),
            convo_id: SharedString::from("#all"),
            sender: SharedString::from("Station-Alpha"),
            text: SharedString::from("All stations check in. RF noise floor clear on Channel 15."),
            timestamp: SharedString::from("18:14:02"),
            status: SharedString::from("[OK]"),
            is_outgoing: false,
            sender_color: slint::Color::from_argb_u8(255, 129, 140, 248), // Indigo
        },
        ChatMessageItem {
            id: SharedString::from("m2"),
            convo_id: SharedString::from("#all"),
            sender: SharedString::from("Me"),
            text: SharedString::from("Copy Alpha. Monitoring airwaves from sector 4. Signal strength solid."),
            timestamp: SharedString::from("18:15:20"),
            status: SharedString::from("*"),
            is_outgoing: true,
            sender_color: slint::Color::from_argb_u8(255, 56, 189, 248), // Cyan
        },
        ChatMessageItem {
            id: SharedString::from("m3"),
            convo_id: SharedString::from("#all"),
            sender: SharedString::from("Field-Recon-02"),
            text: SharedString::from("Overhearing telemetry beacon. Establishing line-of-sight link."),
            timestamp: SharedString::from("18:16:45"),
            status: SharedString::from("[OK]"),
            is_outgoing: false,
            sender_color: slint::Color::from_argb_u8(255, 192, 132, 252), // Purple
        },
    ];
    let swarm_model: ModelRc<ChatMessageItem> = Rc::new(VecModel::from(swarm_messages)).into();
    ui.set_messages(swarm_model);
    ui.set_active_convo_id(SharedString::from("#all"));
    ui.set_active_alias(SharedString::from("Swarm Broadcast"));
    ui.set_active_trust_state(SharedString::from("verified"));
    ui.set_rx_packets(42);
    ui.set_tx_packets(18);
    ui.set_avg_lqi(195);
    ui.set_local_node_id(SharedString::from("0xBEBD82B4"));
    ui.set_storage_mode(SharedString::from("SD ACTIVE"));
    ui.set_storage_stats(SharedString::from("SRAM: 42/256 KB"));

    // Render State 1: Swarm Broadcast with active messages and stations
    let frame1 = runtime.render(ui.window())?;
    frame1.write_png(&out_dir.join("01_swarm_broadcast.png"))?;

    // Render State 2: Direct Message View (unverified contact)
    ui.set_active_convo_id(SharedString::from("0xCAFE1234"));
    ui.set_active_alias(SharedString::from("Field-Recon-02"));
    ui.set_active_trust_state(SharedString::from("unverified"));

    let dm_messages = vec![
        ChatMessageItem {
            id: SharedString::from("dm1"),
            convo_id: SharedString::from("0xCAFE1234"),
            sender: SharedString::from("Field-Recon-02"),
            text: SharedString::from("Requesting tactical waypoint coordinates via direct link."),
            timestamp: SharedString::from("18:20:10"),
            status: SharedString::from("[OK]"),
            is_outgoing: false,
            sender_color: slint::Color::from_argb_u8(255, 192, 132, 252), // Purple
        },
        ChatMessageItem {
            id: SharedString::from("dm2"),
            convo_id: SharedString::from("0xCAFE1234"),
            sender: SharedString::from("Me"),
            text: SharedString::from("Target coordinate grid: 45.281, -111.450. Awaiting SAS verification."),
            timestamp: SharedString::from("18:21:05"),
            status: SharedString::from("[Q]"),
            is_outgoing: true,
            sender_color: slint::Color::from_argb_u8(255, 56, 189, 248), // Cyan
        },
    ];
    let dm_model: ModelRc<ChatMessageItem> = Rc::new(VecModel::from(dm_messages)).into();
    ui.set_messages(dm_model);

    let frame2 = runtime.render(ui.window())?;
    frame2.write_png(&out_dir.join("02_direct_message_view.png"))?;

    // Render State 3: Out-of-band Verification Modal Open
    ui.set_show_verification_modal(true);
    ui.set_verify_node_id(SharedString::from("0xCAFE1234"));
    ui.set_verify_alias(SharedString::from("Field-Recon-02"));
    ui.set_verify_sas_words(SharedString::from("witch collapse practice feed"));

    let frame3 = runtime.render(ui.window())?;
    frame3.write_png(&out_dir.join("03_verification_modal.png"))?;

    // Render State 4: Compact / Mobile Viewport (640 x 500)
    ui.set_show_verification_modal(false);
    runtime.set_size(ui.window(), (640, 500), 1.0)?;
    let frame4 = runtime.render(ui.window())?;
    frame4.write_png(&out_dir.join("04_compact_window.png"))?;

    Ok(())
}
