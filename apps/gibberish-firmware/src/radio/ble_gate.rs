//! Visual LCD Security Gate & Physical Button Authorization for BLE (R9, R10, AE4).
//!
//! When an unbonded BLE client attempts connection:
//! 1. Dongle generates a dynamic 6-digit PIN (e.g. 100000..999999).
//! 2. Renders PIN on ST7735 LCD and flashes APA102 in amber.
//! 3. Requires a physical press of BOOT button (GPIO 28) within 30 seconds.
//! 4. If button is not pressed, pairing request is dropped.

pub const BLE_AUTH_TIMEOUT_MS: u32 = 30_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BleAuthState {
    Idle,
    PairingRequested { pin: u32, remaining_ms: u32 },
    Authorized,
    Rejected,
}

pub struct BleSecurityGate {
    state: BleAuthState,
    bonded: bool,
}

impl BleSecurityGate {
    pub const fn new() -> Self {
        Self {
            state: BleAuthState::Idle,
            bonded: false,
        }
    }

    pub fn is_bonded(&self) -> bool {
        self.bonded
    }

    pub fn state(&self) -> BleAuthState {
        self.state
    }

    /// Called when an unbonded BLE client initiates pairing
    pub fn request_pairing(&mut self, random_seed: u32) -> u32 {
        let pin = 100_000 + (random_seed % 900_000);
        self.state = BleAuthState::PairingRequested {
            pin,
            remaining_ms: BLE_AUTH_TIMEOUT_MS,
        };
        pin
    }

    /// Advance time-to-live for active pairing prompt
    pub fn tick(&mut self, delta_ms: u32) {
        if let BleAuthState::PairingRequested { pin, remaining_ms } = self.state {
            if delta_ms >= remaining_ms {
                self.state = BleAuthState::Rejected;
            } else {
                self.state = BleAuthState::PairingRequested {
                    pin,
                    remaining_ms: remaining_ms - delta_ms,
                };
            }
        }
    }

    /// Called when the physical button (GPIO 28) is pressed
    pub fn on_button_press(&mut self) -> bool {
        match self.state {
            BleAuthState::PairingRequested { .. } => {
                self.state = BleAuthState::Authorized;
                self.bonded = true;
                true
            }
            _ => false,
        }
    }

    pub fn reset(&mut self) {
        self.state = BleAuthState::Idle;
    }
}
