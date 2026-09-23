//! Slotted Time-Division Multiplexed (TDM) Radio Arbiter (KTD1, R1, R9, R14).
//!
//! ESP32-C5 shares a single 2.4 GHz RF synthesizer across 802.15.4 and BLE.
//! This arbiter schedules a 200ms cycle with 2ms guard bands:
//! - 0..168ms: Slot A (802.15.4 Mesh)
//! - 168..170ms: Guard Band 1 (Synthesizer retune)
//! - 170..198ms: Slot B (BLE Companion Link)
//! - 198..200ms: Guard Band 2 (Synthesizer retune)

pub const TDM_CYCLE_MS: u32 = 200;
pub const SLOT_A_END_MS: u32 = 168;
pub const GUARD_1_END_MS: u32 = 170;
pub const SLOT_B_END_MS: u32 = 198;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RadioSlot {
    Ieee802154Mesh,
    GuardBand,
    BleCompanion,
}

pub struct TdmArbiter {
    current_time_ms: u32,
    usb_desktop_mode: bool,
    current_slot: RadioSlot,
}

impl TdmArbiter {
    pub const fn new() -> Self {
        Self {
            current_time_ms: 0,
            usb_desktop_mode: false,
            current_slot: RadioSlot::Ieee802154Mesh,
        }
    }

    /// Set desktop USB mode: BLE sleeps completely, 100% duty cycle to 802.15.4 (KTD1).
    pub fn set_desktop_mode(&mut self, enabled: bool) {
        self.usb_desktop_mode = enabled;
    }

    pub fn is_desktop_mode(&self) -> bool {
        self.usb_desktop_mode
    }

    /// Advance TDM clock by delta_ms and return the active radio slot.
    pub fn advance(&mut self, delta_ms: u32) -> RadioSlot {
        if self.usb_desktop_mode {
            self.current_slot = RadioSlot::Ieee802154Mesh;
            return self.current_slot;
        }

        self.current_time_ms = (self.current_time_ms + delta_ms) % TDM_CYCLE_MS;

        let slot = if self.current_time_ms < SLOT_A_END_MS {
            RadioSlot::Ieee802154Mesh
        } else if self.current_time_ms < GUARD_1_END_MS {
            RadioSlot::GuardBand
        } else if self.current_time_ms < SLOT_B_END_MS {
            RadioSlot::BleCompanion
        } else {
            RadioSlot::GuardBand
        };

        self.current_slot = slot;
        slot
    }

    pub fn current_slot(&self) -> RadioSlot {
        self.current_slot
    }
}
