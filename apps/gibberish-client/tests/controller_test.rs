use gibberish_client::{
    ChatMessageItem, DesktopIpcTransport, SlintController, UiEvent, UiEventSender,
};
use slint::Model;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

#[test]
fn test_ui_event_channel_bounded_capacity() {
    let queue = Arc::new(Mutex::new(std::collections::VecDeque::with_capacity(
        UiEventSender::CAPACITY,
    )));

    // Test queue limits
    {
        let mut q = queue.lock().unwrap();
        for i in 0..UiEventSender::CAPACITY {
            q.push_back(UiEvent::TelemetryUpdated {
                node_id: "0xBEBD82B4".to_string(),
                storage_mode: "SD ACTIVE".to_string(),
                storage_stats: "SRAM: 40/256 KB".to_string(),
                tx: i as i32,
                rx: 0,
                channel: 15,
                avg_lqi: 100,
                status: "OK".to_string(),
            });
        }
        assert_eq!(q.len(), UiEventSender::CAPACITY);
    }
}

#[test]
fn test_wake_pending_latch_coalescing() {
    let wake_pending = Arc::new(AtomicBool::new(false));

    // First attempt succeeds
    let first = wake_pending.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst);
    assert!(first.is_ok(), "First wake acquisition must succeed");

    // Rapid successive attempts must fail (coalesced!)
    for _ in 0..100 {
        let subsequent =
            wake_pending.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst);
        assert!(subsequent.is_err(), "Subsequent wakes while pending must be suppressed");
    }

    // Resetting latch allows next wake
    wake_pending.store(false, Ordering::SeqCst);
    let after_reset =
        wake_pending.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst);
    assert!(after_reset.is_ok(), "Wake acquisition after reset must succeed");
}

#[test]
fn test_slint_controller_initialization_and_accessors() {
    if let Ok(controller) = SlintController::new() {
        let sender = controller.event_sender();

        // Send a telemetry event
        let res = sender.send(UiEvent::TelemetryUpdated {
            node_id: "0xBEBD82B4".to_string(),
            storage_mode: "SD ACTIVE".to_string(),
            storage_stats: "SRAM: 40/256 KB".to_string(),
            tx: 10,
            rx: 20,
            channel: 15,
            avg_lqi: 200,
            status: "CONNECTED".to_string(),
        });
        assert!(res.is_ok());

        assert_eq!(controller.stations_model().row_count(), 0);
        assert_eq!(controller.messages_model().row_count(), 0);
    }
}

#[test]
fn test_conversation_message_isolation_and_switching() {
    if let Ok(controller) = SlintController::new() {
        let sender = controller.event_sender();

        // Add station
        let _ = sender.send(UiEvent::StationDiscovered {
            node_id: "0xBEBD82B4".to_string(),
            alias: "Alice".to_string(),
            rssi: "-70 dBm".to_string(),
            lqi: "LQI 200".to_string(),
            trust_state: "unverified".to_string(),
        });

        // Add message for #all
        let _ = sender.send(UiEvent::MessageReceived(ChatMessageItem {
            id: "m-swarm-1".into(),
            convo_id: "#all".into(),
            sender: "0xCAFE".into(),
            text: "Hello Swarm!".into(),
            timestamp: "12:00:00".into(),
            status: "*".into(),
            is_outgoing: false,
            sender_color: slint::Color::from_argb_u8(255, 129, 140, 248),
        }));

        // Add message for Alice (0xBEBD82B4)
        let _ = sender.send(UiEvent::MessageReceived(ChatMessageItem {
            id: "m-alice-1".into(),
            convo_id: "0xBEBD82B4".into(),
            sender: "Alice".into(),
            text: "Private secret to you".into(),
            timestamp: "12:01:00".into(),
            status: "[OK]".into(),
            is_outgoing: false,
            sender_color: slint::Color::from_argb_u8(255, 163, 230, 53),
        }));

        controller.process_pending_events();

        // Check history cache holds both
        let cache = controller.history_cache().lock().unwrap();
        let swarm_msgs = cache.get("#all").cloned().unwrap_or_default();
        let alice_msgs = cache.get("0xBEBD82B4").cloned().unwrap_or_default();

        assert_eq!(swarm_msgs.len(), 1);
        assert_eq!(swarm_msgs[0].text, "Hello Swarm!");

        assert_eq!(alice_msgs.len(), 1);
        assert_eq!(alice_msgs[0].text, "Private secret to you");

        // Verify isolation: swarm does not contain Alice's message, Alice does not contain swarm
        assert!(!swarm_msgs.iter().any(|m| m.id == "m-alice-1"));
        assert!(!alice_msgs.iter().any(|m| m.id == "m-swarm-1"));
    }
}

#[tokio::test]
async fn test_ipc_transport_client_methods_and_outbound() {
    let ui_sender = UiEventSender::mock();
    let transport = DesktopIpcTransport::new("ws://127.0.0.1:4483", ui_sender);

    // Call all JSON-RPC client methods
    assert!(transport.send_swarm("Swarm broadcast test").is_ok());
    assert!(transport.send_dm(0xBEBD82B4, "Secret DM test").is_ok());
    assert!(transport.verify_contact(0xBEBD82B4, true).is_ok());
    assert!(transport.list_contacts().is_ok());
    assert!(transport.list_messages("#all", 50, 0).is_ok());
}

#[test]
fn test_unread_counter_inactive_increment_and_active_suppression() {
    if let Ok(controller) = SlintController::new() {
        let sender = controller.event_sender();
        let ui = controller.ui();

        // 1. Initially active conversation is "#all"
        assert_eq!(ui.get_active_convo_id(), "#all");
        assert_eq!(ui.get_swarm_unread_count(), 0);

        // 2. Incoming message for active conversation (#all) must NOT increment unread badge
        let _ = sender.send(UiEvent::MessageReceived(ChatMessageItem {
            id: "m-1".into(),
            convo_id: "#all".into(),
            sender: "Alpha".into(),
            text: "Broadcast 1".into(),
            timestamp: "12:00".into(),
            status: "*".into(),
            is_outgoing: false,
            sender_color: slint::Color::from_argb_u8(255, 10, 185, 129),
        }));
        controller.process_pending_events();

        assert_eq!(ui.get_swarm_unread_count(), 0);
        assert_eq!(controller.messages_model().row_count(), 1);

        // 3. Discover station Bravo (0x9C33)
        let _ = sender.send(UiEvent::StationDiscovered {
            node_id: "0x9C33".into(),
            alias: "Bravo".into(),
            rssi: "-70 dBm".into(),
            lqi: "200".into(),
            trust_state: "verified".into(),
        });
        controller.process_pending_events();
        assert_eq!(controller.stations_model().row_count(), 1);
        assert_eq!(controller.stations_model().row_data(0).unwrap().unread_count, 0);

        // 4. Incoming direct message from Bravo while active conversation is #all -> increment Bravo unread
        let _ = sender.send(UiEvent::MessageReceived(ChatMessageItem {
            id: "m-2".into(),
            convo_id: "0x9C33".into(),
            sender: "Bravo".into(),
            text: "Direct message 1".into(),
            timestamp: "12:01".into(),
            status: "[OK]".into(),
            is_outgoing: false,
            sender_color: slint::Color::from_argb_u8(255, 192, 132, 252),
        }));
        controller.process_pending_events();

        assert_eq!(controller.stations_model().row_data(0).unwrap().unread_count, 1);
        // Messages model must not contain Bravo's message because active is #all
        assert_eq!(controller.messages_model().row_count(), 1);

        // 5. Select Bravo conversation -> unread_count resets to 0
        ui.invoke_select_conversation("0x9C33".into());
        assert_eq!(ui.get_active_convo_id(), "0x9C33");
        assert_eq!(controller.stations_model().row_data(0).unwrap().unread_count, 0);

        // 6. Incoming message to #all while Bravo is active -> swarm_unread_count increments to 1
        let _ = sender.send(UiEvent::MessageReceived(ChatMessageItem {
            id: "m-3".into(),
            convo_id: "#all".into(),
            sender: "Alpha".into(),
            text: "Broadcast 2".into(),
            timestamp: "12:02".into(),
            status: "*".into(),
            is_outgoing: false,
            sender_color: slint::Color::from_argb_u8(255, 10, 185, 129),
        }));
        controller.process_pending_events();

        assert_eq!(ui.get_swarm_unread_count(), 1);

        // 7. Select #all -> swarm_unread_count clears to 0
        ui.invoke_select_conversation("#all".into());
        assert_eq!(ui.get_active_convo_id(), "#all");
        assert_eq!(ui.get_swarm_unread_count(), 0);
    }
}
