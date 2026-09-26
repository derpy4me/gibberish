use crate::identity::{derive_sas_words, format_sas_words, generate_verification_qr_svg};
use crate::transport::DesktopIpcTransport;
use gibberish_crypto::ratchet::KeyPair;
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

slint::include_modules!();

#[derive(Debug, Clone)]
pub enum UiEvent {
    StationDiscovered {
        node_id: String,
        alias: String,
        rssi: String,
        lqi: String,
        trust_state: String,
    },
    StationMetricsUpdated {
        node_id: String,
        rssi: String,
        lqi: String,
    },
    StationTrustChanged {
        node_id: String,
        trust_state: String,
    },
    MessageReceived(ChatMessageItem),
    MessageStatusUpdated {
        id: String,
        status: String,
    },
    TelemetryUpdated {
        node_id: String,
        storage_mode: String,
        storage_stats: String,
        tx: i32,
        rx: i32,
        channel: i32,
        avg_lqi: i32,
        status: String,
    },
    ShowVerificationModal {
        node_id: String,
        alias: String,
        sas_words: String,
        qr_svg: Option<String>,
    },
    HideVerificationModal,
}

type ControllerModels = (
    Rc<VecModel<StationItem>>,
    Rc<VecModel<ChatMessageItem>>,
    Arc<Mutex<HashMap<String, Vec<ChatMessageItem>>>>,
    Arc<Mutex<HashMap<String, [u8; 32]>>>,
);

thread_local! {
    static MODELS: std::cell::RefCell<Option<ControllerModels>> = const { std::cell::RefCell::new(None) };
}

#[derive(Clone)]
pub struct UiEventSender {
    queue: Arc<Mutex<VecDeque<UiEvent>>>,
    wake_pending: Arc<AtomicBool>,
    weak_ui: slint::Weak<MainWindow>,
}

impl UiEventSender {
    pub const CAPACITY: usize = 512;

    pub fn mock() -> Self {
        Self {
            queue: Arc::new(Mutex::new(VecDeque::new())),
            wake_pending: Arc::new(AtomicBool::new(false)),
            weak_ui: slint::Weak::default(),
        }
    }

    pub fn send(&self, event: UiEvent) -> Result<(), &'static str> {
        {
            let mut q = self.queue.lock().map_err(|_| "Lock poisoned")?;
            if q.len() >= Self::CAPACITY {
                return Err("UI event channel capacity exceeded");
            }
            q.push_back(event);
        }

        // Coalesce wakeups using AtomicBool guard
        if self
            .wake_pending
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            let weak = self.weak_ui.clone();
            let wake_pending = self.wake_pending.clone();
            let queue = self.queue.clone();

            let res = slint::invoke_from_event_loop(move || {
                if let Some(ui) = weak.upgrade() {
                    drain_and_apply_queue(&ui, &queue, &wake_pending);
                } else {
                    log::warn!("MainWindow weak upgrade failed: window dropped or backgrounded");
                    wake_pending.store(false, Ordering::SeqCst);
                }
            });
            if res.is_err() {
                self.wake_pending.store(false, Ordering::SeqCst);
            }
        }

        Ok(())
    }
}

pub fn drain_and_apply_queue(
    ui: &MainWindow,
    queue: &Arc<Mutex<VecDeque<UiEvent>>>,
    wake_pending: &Arc<AtomicBool>,
) {
    loop {
        let mut events = Vec::new();
        if let Ok(mut q) = queue.lock() {
            while let Some(ev) = q.pop_front() {
                events.push(ev);
            }
        }

        if events.is_empty() {
            wake_pending.store(false, Ordering::SeqCst);
            // Double check if events arrived just before clearing flag
            if let Ok(q) = queue.lock() {
                if !q.is_empty() && wake_pending.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
                    continue;
                }
            }
            break;
        }

        MODELS.with(|m| {
            if let Some((stations_model, messages_model, history_cache, pubkey_cache)) =
                m.borrow().as_ref()
            {
                for ev in events {
                    apply_event(
                        ui,
                        stations_model,
                        messages_model,
                        history_cache,
                        pubkey_cache,
                        ev,
                    );
                }
            }
        });
    }
}

fn apply_event(
    ui: &MainWindow,
    stations_model: &Rc<VecModel<StationItem>>,
    messages_model: &Rc<VecModel<ChatMessageItem>>,
    history_cache: &Arc<Mutex<HashMap<String, Vec<ChatMessageItem>>>>,
    _pubkey_cache: &Arc<Mutex<HashMap<String, [u8; 32]>>>,
    event: UiEvent,
) {
    match event {
        UiEvent::StationDiscovered {
            node_id,
            alias,
            rssi,
            lqi,
            trust_state,
        } => {
            // Check if station already exists
            let mut found = false;
            for i in 0..stations_model.row_count() {
                if let Some(row) = stations_model.row_data(i) {
                    if row.node_id == node_id.as_str() {
                        let mut updated = row;
                        updated.alias = alias.as_str().into();
                        updated.rssi = rssi.as_str().into();
                        updated.lqi = lqi.as_str().into();
                        updated.trust_state = trust_state.as_str().into();
                        updated.signal_bars = compute_signal_bars(&rssi);
                        stations_model.set_row_data(i, updated);
                        found = true;
                        break;
                    }
                }
            }
            if !found {
                stations_model.push(StationItem {
                    node_id: node_id.as_str().into(),
                    alias: alias.as_str().into(),
                    rssi: rssi.as_str().into(),
                    lqi: lqi.as_str().into(),
                    trust_state: trust_state.as_str().into(),
                    signal_bars: compute_signal_bars(&rssi),
                    selected: false,
                });
            }
        }
        UiEvent::StationMetricsUpdated { node_id, rssi, lqi } => {
            for i in 0..stations_model.row_count() {
                if let Some(row) = stations_model.row_data(i) {
                    if row.node_id == node_id.as_str() {
                        let mut updated = row;
                        updated.rssi = rssi.as_str().into();
                        updated.lqi = lqi.as_str().into();
                        updated.signal_bars = compute_signal_bars(&rssi);
                        stations_model.set_row_data(i, updated);
                        break;
                    }
                }
            }
        }
        UiEvent::StationTrustChanged {
            node_id,
            trust_state,
        } => {
            for i in 0..stations_model.row_count() {
                if let Some(row) = stations_model.row_data(i) {
                    if row.node_id == node_id.as_str() {
                        let mut updated = row;
                        updated.trust_state = trust_state.as_str().into();
                        stations_model.set_row_data(i, updated);
                        break;
                    }
                }
            }
            if ui.get_active_convo_id() == node_id.as_str() {
                ui.set_active_trust_state(trust_state.as_str().into());
            }
        }
        UiEvent::MessageReceived(msg) => {
            let convo_id = msg.convo_id.to_string();
            // Cache in history per conversation thread
            if let Ok(mut hist) = history_cache.lock() {
                let list = hist.entry(convo_id.clone()).or_default();
                // Prevent duplicate insertions
                if !list.iter().any(|m| m.id == msg.id) {
                    list.push(msg.clone());
                }
            }
            // Display only if this message belongs to the active conversation thread
            if ui.get_active_convo_id() == convo_id.as_str() {
                let mut exists = false;
                for i in 0..messages_model.row_count() {
                    if let Some(m) = messages_model.row_data(i) {
                        if m.id == msg.id {
                            exists = true;
                            break;
                        }
                    }
                }
                if !exists {
                    messages_model.push(msg);
                }
            }
        }
        UiEvent::MessageStatusUpdated { id, status } => {
            // Update in history cache
            if let Ok(mut hist) = history_cache.lock() {
                for msgs in hist.values_mut() {
                    for m in msgs.iter_mut() {
                        if m.id == id.as_str() {
                            m.status = status.as_str().into();
                        }
                    }
                }
            }
            // Update in currently visible messages model
            for i in 0..messages_model.row_count() {
                if let Some(row) = messages_model.row_data(i) {
                    if row.id == id.as_str() {
                        let mut updated = row;
                        updated.status = status.as_str().into();
                        messages_model.set_row_data(i, updated);
                        break;
                    }
                }
            }
        }
        UiEvent::TelemetryUpdated {
            node_id,
            storage_mode,
            storage_stats,
            tx,
            rx,
            channel,
            avg_lqi,
            status,
        } => {
            ui.set_local_node_id(node_id.as_str().into());
            ui.set_storage_mode(storage_mode.as_str().into());
            ui.set_storage_stats(storage_stats.as_str().into());
            ui.set_tx_packets(tx);
            ui.set_rx_packets(rx);
            ui.set_mesh_channel(channel);
            ui.set_avg_lqi(avg_lqi);
            ui.set_dongle_status(status.as_str().into());
        }
        UiEvent::ShowVerificationModal {
            node_id,
            alias,
            sas_words,
            qr_svg,
        } => {
            ui.set_verify_node_id(node_id.into());
            ui.set_verify_alias(alias.into());
            ui.set_verify_sas_words(sas_words.into());
            if let Some(svg) = qr_svg {
                if let Ok(img) = slint::Image::load_from_svg_data(svg.as_bytes()) {
                    ui.set_verify_qr_image(img);
                }
            }
            ui.set_show_verification_modal(true);
        }
        UiEvent::HideVerificationModal => {
            ui.set_show_verification_modal(false);
        }
    }
}

pub struct SlintController {
    ui: MainWindow,
    event_sender: UiEventSender,
    stations_model: Rc<VecModel<StationItem>>,
    messages_model: Rc<VecModel<ChatMessageItem>>,
    history_cache: Arc<Mutex<HashMap<String, Vec<ChatMessageItem>>>>,
    pubkey_cache: Arc<Mutex<HashMap<String, [u8; 32]>>>,
    transport: Arc<Mutex<Option<DesktopIpcTransport>>>,
}

impl SlintController {
    pub fn new() -> Result<Self, slint::PlatformError> {
        let ui = MainWindow::new()?;

        let stations_model = Rc::new(VecModel::<StationItem>::default());
        let messages_model = Rc::new(VecModel::<ChatMessageItem>::default());
        let history_cache = Arc::new(Mutex::new(HashMap::<String, Vec<ChatMessageItem>>::new()));
        let pubkey_cache = Arc::new(Mutex::new(HashMap::<String, [u8; 32]>::new()));
        let transport = Arc::new(Mutex::new(None::<DesktopIpcTransport>));

        ui.set_stations(ModelRc::from(stations_model.clone()));
        ui.set_messages(ModelRc::from(messages_model.clone()));

        MODELS.with(|m| {
            *m.borrow_mut() = Some((
                stations_model.clone(),
                messages_model.clone(),
                history_cache.clone(),
                pubkey_cache.clone(),
            ));
        });

        let queue = Arc::new(Mutex::new(VecDeque::with_capacity(UiEventSender::CAPACITY)));
        let wake_pending = Arc::new(AtomicBool::new(false));

        let event_sender = UiEventSender {
            queue,
            wake_pending,
            weak_ui: ui.as_weak(),
        };

        // Wire UI callbacks:
        // 1. Select conversation
        let stations_clone = stations_model.clone();
        let messages_clone = messages_model.clone();
        let history_clone = history_cache.clone();
        let transport_clone = transport.clone();
        let ui_weak = ui.as_weak();
        ui.on_select_conversation(move |id| {
            if let Some(ui) = ui_weak.upgrade() {
                let id_str = id.to_string();
                ui.set_active_convo_id(id_str.clone().into());

                if id_str == "#all" {
                    ui.set_active_alias("Swarm Broadcast".into());
                    ui.set_active_trust_state("verified".into());
                } else {
                    for i in 0..stations_clone.row_count() {
                        if let Some(row) = stations_clone.row_data(i) {
                            if row.node_id == id_str.as_str() {
                                ui.set_active_alias(row.alias.clone());
                                ui.set_active_trust_state(row.trust_state.clone());
                                break;
                            }
                        }
                    }
                }

                // Update selected flag on roster
                for i in 0..stations_clone.row_count() {
                    if let Some(mut row) = stations_clone.row_data(i) {
                        row.selected = row.node_id == id_str.as_str();
                        stations_clone.set_row_data(i, row);
                    }
                }

                // Switch active conversation thread messages
                let convo_msgs = if let Ok(hist) = history_clone.lock() {
                    hist.get(&id_str).cloned().unwrap_or_default()
                } else {
                    Vec::new()
                };
                messages_clone.set_vec(convo_msgs);

                // Fetch message history from daemon if transport available
                if let Ok(guard) = transport_clone.lock() {
                    if let Some(t) = guard.as_ref() {
                        let _ = t.list_messages(&id_str, 50, 0);
                    }
                }
            }
        });

        // 2. Send chat message
        let transport_send = transport.clone();
        let sender_for_chat = event_sender.clone();
        let ui_weak_send = ui.as_weak();
        ui.on_send_chat_message(move |txt| {
            if let Some(ui) = ui_weak_send.upgrade() {
                let convo = ui.get_active_convo_id().to_string();
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);

                let mut sent_via_ipc = false;
                if let Ok(guard) = transport_send.lock() {
                    if let Some(t) = guard.as_ref() {
                        if convo == "#all" {
                            sent_via_ipc = t.send_swarm(txt.as_str()).is_ok();
                        } else {
                            let clean = convo.trim_start_matches("0x");
                            if let Ok(dest_id) = u32::from_str_radix(clean, 16) {
                                sent_via_ipc = t.send_dm(dest_id, txt.as_str()).is_ok();
                            }
                        }
                    }
                }

                // In standalone or test mode without daemon, reflect message locally
                if !sent_via_ipc {
                    let status = if convo == "#all" { "*" } else { "[Q]" };
                    let is_outgoing = true;
                    let _ = sender_for_chat.send(UiEvent::MessageReceived(ChatMessageItem {
                        id: format!("local-{}", now).into(),
                        convo_id: convo.into(),
                        sender: "Me".into(),
                        text: txt,
                        timestamp: format!("{:02}:{:02}:{:02}", (now % 86400) / 3600, (now % 3600) / 60, now % 60).into(),
                        status: status.into(),
                        is_outgoing,
                        sender_color: derive_sender_color("Me", is_outgoing),
                    }));
                }
            }
        });

        // 3. Request verification modal
        let sender_for_verify = event_sender.clone();
        let stations_for_verify = stations_model.clone();
        let pubkeys_for_verify = pubkey_cache.clone();
        ui.on_request_verification(move |node_id| {
            let node_id_str = node_id.to_string();
            let mut alias_str = node_id_str.clone();

            for i in 0..stations_for_verify.row_count() {
                if let Some(row) = stations_for_verify.row_data(i) {
                    if row.node_id == node_id_str.as_str() {
                        alias_str = row.alias.to_string();
                        break;
                    }
                }
            }

            let clean = node_id_str.trim_start_matches("0x");
            let node_id_u32 = u32::from_str_radix(clean, 16).unwrap_or(0);

            // Fetch or derive peer pubkey
            let peer_pubkey = if let Ok(pk_map) = pubkeys_for_verify.lock() {
                pk_map.get(&node_id_str).copied().unwrap_or_else(|| {
                    let mut pk = [0u8; 32];
                    let bytes = node_id_u32.to_be_bytes();
                    pk[0..4].copy_from_slice(&bytes);
                    pk
                })
            } else {
                [0u8; 32]
            };

            let local_keypair = KeyPair::from_secret_bytes([0x42; 32]);
            let sas_words = derive_sas_words(local_keypair.public_key(), &peer_pubkey);
            let sas_formatted = format_sas_words(&sas_words);
            let qr_svg = generate_verification_qr_svg(node_id_u32, &alias_str, &peer_pubkey).ok();

            let _ = sender_for_verify.send(UiEvent::ShowVerificationModal {
                node_id: node_id_str,
                alias: alias_str,
                sas_words: sas_formatted,
                qr_svg,
            });
        });

        // 4. Confirm verification
        let sender_for_confirm = event_sender.clone();
        let transport_confirm = transport.clone();
        ui.on_confirm_verification(move |node_id| {
            let node_id_str = node_id.to_string();
            let clean = node_id_str.trim_start_matches("0x");
            if let Ok(dest_id) = u32::from_str_radix(clean, 16) {
                if let Ok(guard) = transport_confirm.lock() {
                    if let Some(t) = guard.as_ref() {
                        let _ = t.verify_contact(dest_id, true);
                    }
                }
            }
            let _ = sender_for_confirm.send(UiEvent::StationTrustChanged {
                node_id: node_id_str,
                trust_state: "verified".to_string(),
            });
            let _ = sender_for_confirm.send(UiEvent::HideVerificationModal);
        });

        // 5. Close verification modal
        let sender_for_close = event_sender.clone();
        ui.on_close_verification(move || {
            let _ = sender_for_close.send(UiEvent::HideVerificationModal);
        });

        Ok(Self {
            ui,
            event_sender,
            stations_model,
            messages_model,
            history_cache,
            pubkey_cache,
            transport,
        })
    }

    pub fn set_transport(&self, transport: DesktopIpcTransport) {
        if let Ok(mut guard) = self.transport.lock() {
            *guard = Some(transport.clone());
        }
        // Fetch contact list on attachment
        let _ = transport.list_contacts();
    }

    pub fn register_contact_pubkey(&self, node_id: &str, pubkey: [u8; 32]) {
        if let Ok(mut map) = self.pubkey_cache.lock() {
            map.insert(node_id.to_string(), pubkey);
        }
    }

    pub fn event_sender(&self) -> UiEventSender {
        self.event_sender.clone()
    }

    pub fn window(&self) -> &MainWindow {
        &self.ui
    }

    pub fn stations_model(&self) -> &Rc<VecModel<StationItem>> {
        &self.stations_model
    }

    pub fn messages_model(&self) -> &Rc<VecModel<ChatMessageItem>> {
        &self.messages_model
    }

    pub fn history_cache(&self) -> &Arc<Mutex<HashMap<String, Vec<ChatMessageItem>>>> {
        &self.history_cache
    }

    pub fn process_pending_events(&self) {
        drain_and_apply_queue(
            &self.ui,
            &self.event_sender.queue,
            &self.event_sender.wake_pending,
        );
    }

    pub fn run(self) -> Result<(), slint::PlatformError> {
        self.ui.run()
    }
}

/// Derives a deterministic color from the 5-color peer palette:
/// 1. #a3e635 (Chartreuse / Tactical Lime)
/// 2. #818cf8 (Indigo)
/// 3. #c084fc (Purple)
/// 4. #e879f9 (Fuchsia)
/// 5. #f472b6 (Rose Pink)
/// If outgoing (<Me>), returns Cyan #38bdf8.
pub fn derive_sender_color(sender: &str, is_outgoing: bool) -> slint::Color {
    if is_outgoing {
        slint::Color::from_argb_u8(255, 0x38, 0xbd, 0xf8)
    } else {
        let mut h: u32 = 0;
        for b in sender.bytes() {
            h = h.wrapping_mul(31).wrapping_add(b as u32);
        }
        match h % 5 {
            0 => slint::Color::from_argb_u8(255, 0xa3, 0xe6, 0x35),
            1 => slint::Color::from_argb_u8(255, 0x81, 0x8c, 0xf8),
            2 => slint::Color::from_argb_u8(255, 0xc0, 0x84, 0xfc),
            3 => slint::Color::from_argb_u8(255, 0xe8, 0x79, 0xf9),
            _ => slint::Color::from_argb_u8(255, 0xf4, 0x72, 0xb6),
        }
    }
}

pub fn compute_signal_bars(rssi_str: &str) -> i32 {
    let clean = rssi_str.replace("dBm", "").trim().to_string();
    if let Ok(num) = clean.parse::<i32>() {
        if num >= -70 {
            4
        } else if num >= -80 {
            3
        } else if num >= -90 {
            2
        } else {
            1
        }
    } else {
        3
    }
}

