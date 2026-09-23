//! APA102 DotStar RGB LED Driver for LilyGO T-Dongle-C5 (R10).
//!
//! Pinout:
//! - GPIO 4: SPI Clock
//! - GPIO 5: SPI Data

use esp_hal::gpio::Output;

pub struct Apa102<'d> {
    clk: Output<'d>,
    data: Output<'d>,
}

impl<'d> Apa102<'d> {
    pub fn new(clk: Output<'d>, data: Output<'d>) -> Self {
        Self { clk, data }
    }

    fn write_byte(&mut self, byte: u8) {
        for bit in (0..8).rev() {
            if (byte & (1 << bit)) != 0 {
                self.data.set_high();
            } else {
                self.data.set_low();
            }
            self.clk.set_high();
            self.clk.set_low();
        }
    }

    /// Set pixel color and brightness (brightness: 0..31).
    pub fn set_pixel(&mut self, rgb: [u8; 3], brightness: u8) {
        let b = brightness.min(31) | 0xE0;

        // Start frame: 32 zero bits
        for _ in 0..4 {
            self.write_byte(0x00);
        }

        // LED frame: 111[5-bit brightness] + B + G + R
        self.write_byte(b);
        self.write_byte(rgb[2]); // Blue
        self.write_byte(rgb[1]); // Green
        self.write_byte(rgb[0]); // Red

        // End frame: 32 one bits
        for _ in 0..4 {
            self.write_byte(0xFF);
        }
    }

    pub fn set_amber_pulse(&mut self, step: u8) {
        // Pulse brightness 1..15 based on step
        let b = 2 + (step % 12);
        self.set_pixel([255, 140, 0], b);
    }

    pub fn set_green(&mut self) {
        self.set_pixel([0, 255, 0], 5);
    }

    pub fn set_cyan(&mut self) {
        self.set_pixel([0, 255, 255], 8);
    }

    pub fn set_magenta(&mut self) {
        self.set_pixel([255, 0, 255], 8);
    }

    pub fn set_idle_blue(&mut self) {
        self.set_pixel([0, 40, 180], 2);
    }

    pub fn turn_off(&mut self) {
        self.set_pixel([0, 0, 0], 0);
    }
}
