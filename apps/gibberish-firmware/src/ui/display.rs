//! ST7735 LCD Display Driver (160x80) for LilyGO T-Dongle-C5 (R9, R10, R28).

use core::cell::RefCell;
use core::fmt::Write;
use esp_hal::delay::Delay;
use esp_hal::gpio::Output;
use esp_hal::spi::master::Spi;
use esp_hal::Blocking;
use gibberish_protocol::StorageModeStatus;

pub const LCD_WIDTH: u16 = 160;
pub const LCD_HEIGHT: u16 = 80;
pub const COL_OFFSET: u16 = 1;
pub const ROW_OFFSET: u16 = 26;

pub struct St7735<'a, 'd> {
    spi: &'a RefCell<Spi<'d, Blocking>>,
    cs: Output<'d>,
    dc: Output<'d>,
    rst: Output<'d>,
    backlight: Output<'d>,
}

impl<'a, 'd> St7735<'a, 'd> {
    pub fn new(
        spi: &'a RefCell<Spi<'d, Blocking>>,
        cs: Output<'d>,
        dc: Output<'d>,
        rst: Output<'d>,
        backlight: Output<'d>,
    ) -> Self {
        Self {
            spi,
            cs,
            dc,
            rst,
            backlight,
        }
    }

    pub fn set_backlight(&mut self, on: bool) {
        if on {
            self.backlight.set_low(); // Active-low on GPIO 0
        } else {
            self.backlight.set_high();
        }
    }

    fn write_cmd(&mut self, cmd: u8) {
        self.dc.set_low();
        self.cs.set_low();
        let _ = self.spi.borrow_mut().write(&[cmd]);
        self.cs.set_high();
    }

    fn write_data(&mut self, data: &[u8]) {
        self.dc.set_high();
        self.cs.set_low();
        let _ = self.spi.borrow_mut().write(data);
        self.cs.set_high();
    }


    pub fn init(&mut self, delay: &mut Delay) {
        self.backlight.set_high(); // OFF during init

        self.rst.set_high();
        delay.delay_millis(5);
        self.rst.set_low();
        delay.delay_millis(20);
        self.rst.set_high();
        delay.delay_millis(150);

        self.write_cmd(0x01); // SWRESET
        delay.delay_millis(150);

        self.write_cmd(0x11); // SLPOUT
        delay.delay_millis(120);

        self.write_cmd(0xB1); // FRMCTR1
        self.write_data(&[0x01, 0x2C, 0x2D]);

        self.write_cmd(0xB2); // FRMCTR2
        self.write_data(&[0x01, 0x2C, 0x2D]);

        self.write_cmd(0xB3); // FRMCTR3
        self.write_data(&[0x01, 0x2C, 0x2D, 0x01, 0x2C, 0x2D]);

        self.write_cmd(0xB4); // INVCTR
        self.write_data(&[0x07]);

        self.write_cmd(0xC0); // PWCTR1
        self.write_data(&[0xA2, 0x02, 0x84]);
        self.write_cmd(0xC1); // PWCTR2
        self.write_data(&[0xC5]);
        self.write_cmd(0xC2); // PWCTR3
        self.write_data(&[0x0A, 0x00]);
        self.write_cmd(0xC3); // PWCTR4
        self.write_data(&[0x8A, 0x2A]);
        self.write_cmd(0xC4); // PWCTR5
        self.write_data(&[0x8A, 0xEE]);

        self.write_cmd(0xC5); // VMCTR1
        self.write_data(&[0x0E]);

        self.write_cmd(0x21); // INVOFF / INVON
        self.write_cmd(0x3A); // COLMOD
        self.write_data(&[0x05]); // 16-bit RGB565

        self.write_cmd(0x36); // MADCTL: Landscape
        self.write_data(&[0x60]);

        self.write_cmd(0x13); // NORON
        delay.delay_millis(10);

        self.write_cmd(0x29); // DISPON
        delay.delay_millis(100);

        self.fill_rect(0, 0, LCD_WIDTH, LCD_HEIGHT, 0x0000);
        self.backlight.set_low(); // Backlight ON
    }

    pub fn set_window(&mut self, x0: u16, y0: u16, x1: u16, y1: u16) {
        let x_start = x0 + COL_OFFSET;
        let x_end = x1 + COL_OFFSET;
        let y_start = y0 + ROW_OFFSET;
        let y_end = y1 + ROW_OFFSET;

        self.write_cmd(0x2A);
        self.write_data(&[
            (x_start >> 8) as u8,
            (x_start & 0xFF) as u8,
            (x_end >> 8) as u8,
            (x_end & 0xFF) as u8,
        ]);

        self.write_cmd(0x2B);
        self.write_data(&[
            (y_start >> 8) as u8,
            (y_start & 0xFF) as u8,
            (y_end >> 8) as u8,
            (y_end & 0xFF) as u8,
        ]);
    }

    pub fn fill_rect(&mut self, x: u16, y: u16, w: u16, h: u16, color_rgb565: u16) {
        if w == 0 || h == 0 || x >= LCD_WIDTH || y >= LCD_HEIGHT {
            return;
        }
        let x1 = (x + w - 1).min(LCD_WIDTH - 1);
        let y1 = (y + h - 1).min(LCD_HEIGHT - 1);

        self.set_window(x, y, x1, y1);

        let count = ((x1 - x + 1) as usize) * ((y1 - y + 1) as usize);
        let b1 = (color_rgb565 >> 8) as u8;
        let b2 = (color_rgb565 & 0xFF) as u8;

        let mut buf = [0u8; 64];
        for i in 0..32 {
            buf[i * 2] = b1;
            buf[i * 2 + 1] = b2;
        }

        self.cs.set_low();
        self.dc.set_low();
        {
            let mut spi = self.spi.borrow_mut();
            let _ = spi.write(&[0x2C]);
            self.dc.set_high();

            let mut remaining = count;
            while remaining > 0 {
                let chunk = remaining.min(32);
                let _ = spi.write(&buf[..(chunk * 2)]);
                remaining -= chunk;
            }
        }
        self.cs.set_high();
    }

    pub fn draw_char(&mut self, x: u16, y: u16, c: char, fg: u16, bg: u16) {
        if x + 6 > LCD_WIDTH || y + 8 > LCD_HEIGHT {
            return;
        }
        let font_idx = if (c as usize) >= 32 && (c as usize) <= 126 {
            c as usize - 32
        } else {
            0
        };
        let glyph = FONT_5X7[font_idx];

        self.set_window(x, y, x + 5, y + 7);

        let mut buf = [0u8; 6 * 8 * 2];
        let mut idx = 0;

        for row in 0..8 {
            for col in 0..6 {
                let is_fg = if col < 5 && row < 7 {
                    (glyph[col] & (1 << row)) != 0
                } else {
                    false
                };
                let pixel = if is_fg { fg } else { bg };
                buf[idx] = (pixel >> 8) as u8;
                buf[idx + 1] = (pixel & 0xFF) as u8;
                idx += 2;
            }
        }

        self.cs.set_low();
        self.dc.set_low();
        {
            let mut spi = self.spi.borrow_mut();
            let _ = spi.write(&[0x2C]);
            self.dc.set_high();
            let _ = spi.write(&buf);
        }
        self.cs.set_high();
    }


    pub fn draw_text(&mut self, mut x: u16, y: u16, text: &str, fg: u16, bg: u16) {
        for c in text.chars() {
            if x + 6 > LCD_WIDTH {
                break;
            }
            self.draw_char(x, y, c, fg, bg);
            x += 6;
        }
    }

    /// High-level Gibberish dashboard status render (4 Hz update)
    pub fn render_status(
        &mut self,
        storage_mode: StorageModeStatus,
        rx_count: u32,
        tx_count: u32,
        sram_used: usize,
        dropped: u32,
        ble_pin: Option<u32>,
        btn_active: bool,
    ) {
        // Line 0: Header Banner
        self.draw_text(4, 4, "=== GIBBERISH MESH ===", 0x07FF, 0x0000); // Cyan

        // Line 1: Storage Status (Yellow if RAM only, Green if SD active)
        let (sd_text, sd_color) = match storage_mode {
            StorageModeStatus::RamOnly => ("SD: NONE <RAM ONLY>", 0xFFE0), // Yellow
            StorageModeStatus::MicroSdActive => ("SD: FAT32 ACTIVE  ", 0x07E0), // Green
        };
        self.draw_text(4, 18, sd_text, sd_color, 0x0000);

        // Line 2: Mesh RX/TX Counts
        let mut line2 = StrBuf::<32>::new();
        let _ = write!(line2, "RX:{:<5} TX:{:<5}", rx_count, tx_count);
        self.draw_text(4, 32, line2.as_str(), 0xFFFF, 0x0000);

        // Line 3: SRAM Ring & Drops
        let mut line3 = StrBuf::<32>::new();
        let _ = write!(line3, "RAM:{}/256 DRP:{}", sram_used, dropped);
        self.draw_text(4, 46, line3.as_str(), 0xCE79, 0x0000);

        // Line 4: BLE Companion / PIN Prompt
        if let Some(pin) = ble_pin {
            let mut pin_line = StrBuf::<32>::new();
            let _ = write!(pin_line, "PAIR PIN: {:06}", pin);
            self.draw_text(4, 60, pin_line.as_str(), 0xFD20, 0x0000); // Orange/Amber
        } else if btn_active {
            self.draw_text(4, 60, "BTN: BEACON SENT!    ", 0x07E0, 0x0000); // Bright Green
        } else {
            self.draw_text(4, 60, "BLE: READY [TAP BOOT]", 0xAD55, 0x0000);
        }
    }
}

pub struct StrBuf<const N: usize> {
    buf: [u8; N],
    len: usize,
}

impl<const N: usize> StrBuf<N> {
    pub const fn new() -> Self {
        Self {
            buf: [0u8; N],
            len: 0,
        }
    }

    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.buf[..self.len]).unwrap_or("")
    }
}

impl<const N: usize> Write for StrBuf<N> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let bytes = s.as_bytes();
        let avail = N - self.len;
        let to_copy = bytes.len().min(avail);
        self.buf[self.len..self.len + to_copy].copy_from_slice(&bytes[..to_copy]);
        self.len += to_copy;
        Ok(())
    }
}

// Compact 5x7 ASCII font table (ASCII 32 to 126)
const FONT_5X7: [[u8; 5]; 95] = [
    [0x00, 0x00, 0x00, 0x00, 0x00], // Space (32)
    [0x00, 0x00, 0x5F, 0x00, 0x00], // !
    [0x00, 0x07, 0x00, 0x07, 0x00], // "
    [0x14, 0x7F, 0x14, 0x7F, 0x14], // #
    [0x24, 0x2A, 0x7F, 0x2A, 0x12], // $
    [0x23, 0x13, 0x08, 0x64, 0x62], // %
    [0x36, 0x49, 0x55, 0x22, 0x50], // &
    [0x00, 0x05, 0x03, 0x00, 0x00], // '
    [0x00, 0x1C, 0x22, 0x41, 0x00], // (
    [0x00, 0x41, 0x22, 0x1C, 0x00], // )
    [0x14, 0x08, 0x3E, 0x08, 0x14], // *
    [0x08, 0x08, 0x3E, 0x08, 0x08], // +
    [0x00, 0x50, 0x30, 0x00, 0x00], // ,
    [0x08, 0x08, 0x08, 0x08, 0x08], // -
    [0x00, 0x60, 0x60, 0x00, 0x00], // .
    [0x20, 0x10, 0x08, 0x04, 0x02], // /
    [0x3E, 0x51, 0x49, 0x45, 0x3E], // 0
    [0x00, 0x42, 0x7F, 0x40, 0x00], // 1
    [0x42, 0x61, 0x51, 0x49, 0x46], // 2
    [0x21, 0x41, 0x45, 0x4B, 0x31], // 3
    [0x18, 0x14, 0x12, 0x7F, 0x10], // 4
    [0x27, 0x45, 0x45, 0x45, 0x39], // 5
    [0x3C, 0x4A, 0x49, 0x49, 0x30], // 6
    [0x01, 0x71, 0x09, 0x05, 0x03], // 7
    [0x36, 0x49, 0x49, 0x49, 0x36], // 8
    [0x06, 0x49, 0x49, 0x29, 0x1E], // 9
    [0x00, 0x36, 0x36, 0x00, 0x00], // :
    [0x00, 0x56, 0x36, 0x00, 0x00], // ;
    [0x08, 0x14, 0x22, 0x41, 0x00], // <
    [0x14, 0x14, 0x14, 0x14, 0x14], // =
    [0x00, 0x41, 0x22, 0x14, 0x08], // >
    [0x02, 0x01, 0x51, 0x09, 0x06], // ?
    [0x32, 0x49, 0x79, 0x41, 0x3E], // @
    [0x7E, 0x11, 0x11, 0x11, 0x7E], // A
    [0x7F, 0x49, 0x49, 0x49, 0x36], // B
    [0x3E, 0x41, 0x41, 0x41, 0x22], // C
    [0x7F, 0x41, 0x41, 0x22, 0x1C], // D
    [0x7F, 0x49, 0x49, 0x49, 0x41], // E
    [0x7F, 0x09, 0x09, 0x09, 0x01], // F
    [0x3E, 0x41, 0x49, 0x49, 0x7A], // G
    [0x7F, 0x08, 0x08, 0x08, 0x7F], // H
    [0x00, 0x41, 0x7F, 0x41, 0x00], // I
    [0x20, 0x40, 0x41, 0x3F, 0x01], // J
    [0x7F, 0x08, 0x14, 0x22, 0x41], // K
    [0x7F, 0x40, 0x40, 0x40, 0x40], // L
    [0x7F, 0x02, 0x0C, 0x02, 0x7F], // M
    [0x7F, 0x04, 0x08, 0x10, 0x7F], // N
    [0x3E, 0x41, 0x41, 0x41, 0x3E], // O
    [0x7F, 0x09, 0x09, 0x09, 0x06], // P
    [0x3E, 0x41, 0x51, 0x21, 0x5E], // Q
    [0x7F, 0x09, 0x19, 0x29, 0x46], // R
    [0x46, 0x49, 0x49, 0x49, 0x31], // S
    [0x01, 0x01, 0x7F, 0x01, 0x01], // T
    [0x3F, 0x40, 0x40, 0x40, 0x3F], // U
    [0x1F, 0x20, 0x40, 0x20, 0x1F], // V
    [0x3F, 0x40, 0x38, 0x40, 0x3F], // W
    [0x63, 0x14, 0x08, 0x14, 0x63], // X
    [0x07, 0x08, 0x70, 0x08, 0x07], // Y
    [0x61, 0x51, 0x49, 0x45, 0x43], // Z
    [0x00, 0x7F, 0x41, 0x41, 0x00], // [
    [0x02, 0x04, 0x08, 0x10, 0x20], // \
    [0x00, 0x41, 0x41, 0x7F, 0x00], // ]
    [0x04, 0x02, 0x01, 0x02, 0x04], // ^
    [0x40, 0x40, 0x40, 0x40, 0x40], // _
    [0x00, 0x01, 0x02, 0x04, 0x00], // `
    [0x20, 0x54, 0x54, 0x54, 0x78], // a
    [0x7F, 0x48, 0x44, 0x44, 0x38], // b
    [0x38, 0x44, 0x44, 0x44, 0x20], // c
    [0x38, 0x44, 0x44, 0x48, 0x7F], // d
    [0x38, 0x54, 0x54, 0x54, 0x18], // e
    [0x08, 0x7E, 0x09, 0x01, 0x02], // f
    [0x0C, 0x52, 0x52, 0x52, 0x3E], // g
    [0x7F, 0x08, 0x04, 0x04, 0x78], // h
    [0x00, 0x44, 0x7D, 0x40, 0x00], // i
    [0x20, 0x40, 0x44, 0x3D, 0x00], // j
    [0x7F, 0x10, 0x28, 0x44, 0x00], // k
    [0x00, 0x41, 0x7F, 0x40, 0x00], // l
    [0x7C, 0x04, 0x18, 0x04, 0x78], // m
    [0x7C, 0x08, 0x04, 0x04, 0x78], // n
    [0x38, 0x44, 0x44, 0x44, 0x38], // o
    [0x7C, 0x14, 0x14, 0x14, 0x08], // p
    [0x08, 0x14, 0x14, 0x18, 0x7C], // q
    [0x7C, 0x08, 0x04, 0x04, 0x08], // r
    [0x48, 0x54, 0x54, 0x54, 0x20], // s
    [0x04, 0x3F, 0x44, 0x40, 0x20], // t
    [0x3C, 0x40, 0x40, 0x20, 0x7C], // u
    [0x1C, 0x20, 0x40, 0x20, 0x1C], // v
    [0x3C, 0x40, 0x30, 0x40, 0x3C], // w
    [0x44, 0x28, 0x10, 0x28, 0x44], // x
    [0x0C, 0x50, 0x50, 0x50, 0x3C], // y
    [0x44, 0x64, 0x54, 0x4C, 0x44], // z
    [0x00, 0x08, 0x36, 0x41, 0x00], // {
    [0x00, 0x00, 0x7F, 0x00, 0x00], // |
    [0x00, 0x41, 0x36, 0x08, 0x00], // }
    [0x08, 0x08, 0x2A, 0x1C, 0x08], // ~
];
