//! Dynamic MicroSD card driver and Ephemeral RAM-Only fallback manager (R7, R8, R26, R28, R29).

use core::cell::RefCell;
use esp_hal::delay::Delay;
use esp_hal::gpio::Output;
use esp_hal::spi::master::{Config as SpiConfig, Spi};
use esp_hal::spi::Mode;
use esp_hal::time::Rate;
use esp_hal::Blocking;

pub use gibberish_storage::DynamicStorageManager;
use gibberish_storage::fat32_container::{crc7, BlockDevice, StorageError, SECTOR_SIZE};

/// Send 6-byte SD SPI command with standard CRC-7 checksum
fn send_cmd(spi: &mut Spi<'_, Blocking>, cmd: u8, arg: u32) {
    let arg_bytes = arg.to_be_bytes();
    let mut cmd_buf = [
        0x40 | (cmd & 0x3F),
        arg_bytes[0],
        arg_bytes[1],
        arg_bytes[2],
        arg_bytes[3],
        0,
    ];
    cmd_buf[5] = crc7(&cmd_buf[0..5]);
    let _ = spi.write(&cmd_buf);
}

/// Poll for R1 response byte (MSB is 0)
fn wait_r1(spi: &mut Spi<'_, Blocking>, max_attempts: usize) -> Result<u8, StorageError> {
    for _ in 0..max_attempts {
        let mut b = [0xFF];
        let _ = spi.transfer(&mut b);
        if (b[0] & 0x80) == 0 {
            return Ok(b[0]);
        }
    }
    Err(StorageError::DeviceError)
}

/// Helper to restore shared SPI bus to 16 MHz operational frequency
fn restore_spi_16m(spi: &RefCell<Spi<'_, Blocking>>) {
    let cfg_16m = SpiConfig::default()
        .with_frequency(Rate::from_mhz(16))
        .with_mode(Mode::_0);
    let _ = spi.borrow_mut().apply_config(&cfg_16m);
}

/// Physical SPI MicroSD Block Device Driver
pub struct SpiSdBlockDevice<'a, 'd> {
    spi: &'a RefCell<Spi<'d, Blocking>>,
    cs: Output<'d>,
    is_sdhc: bool,
}

impl<'a, 'd> SpiSdBlockDevice<'a, 'd> {
    /// Probe and initialize MicroSD card over shared SPI2 bus.
    /// Fast timeout (<50ms) ensures smooth fallback to RAM-Only on cardless dongles.
    /// In all exit paths (success or error), ensures shared SPI bus is restored to 16 MHz.
    pub fn new(
        spi: &'a RefCell<Spi<'d, Blocking>>,
        mut cs: Output<'d>,
        delay: &mut Delay,
    ) -> Result<Self, StorageError> {
        // Ensure CS is deasserted high initially
        cs.set_high();

        // 1. Configure SPI clock to 400 kHz (<= 400kHz required by SD spec during init)
        let cfg_400k = SpiConfig::default()
            .with_frequency(Rate::from_khz(400))
            .with_mode(Mode::_0);
        if spi.borrow_mut().apply_config(&cfg_400k).is_err() {
            restore_spi_16m(spi);
            return Err(StorageError::DeviceError);
        }

        // Power-on stabilization delay
        delay.delay_millis(10);

        // 2. Clock at least 74 cycles (10 * 8 = 80 clocks) with CS high to wake card internal state machine
        {
            let mut spi_ref = spi.borrow_mut();
            let dummy = [0xFF; 10];
            let _ = spi_ref.write(&dummy);
        }

        // 3. Send CMD0 to enter SPI mode (retry up to 5 times)
        let mut in_idle = false;
        for _ in 0..5 {
            cs.set_low();
            let r1 = {
                let mut spi_ref = spi.borrow_mut();
                send_cmd(&mut spi_ref, 0, 0);
                wait_r1(&mut spi_ref, 50)
            };
            // Deassert CS with trailing dummy clocks to frame CMD0 transaction
            cs.set_high();
            {
                let mut spi_ref = spi.borrow_mut();
                let _ = spi_ref.write(&[0xFF, 0xFF]);
            }

            if let Ok(0x01) = r1 {
                in_idle = true;
                break;
            }
            delay.delay_millis(2);
        }

        if !in_idle {
            // Absent or unresponsive card -> restore 16MHz and abort immediately (<50ms)
            restore_spi_16m(spi);
            return Err(StorageError::DeviceError);
        }

        // 4. Send CMD8 to check SDv2+ support and voltage range
        cs.set_low();
        let mut is_v2 = false;
        let cmd8_ok = {
            let mut spi_ref = spi.borrow_mut();
            send_cmd(&mut spi_ref, 8, 0x000001AA);
            if let Ok(r1) = wait_r1(&mut spi_ref, 100) {
                if r1 == 0x01 {
                    // SDv2+: Read remaining 4 bytes of R7 response
                    let mut r7 = [0xFF; 4];
                    let _ = spi_ref.transfer(&mut r7);
                    if r7[3] == 0xAA && (r7[2] & 0x0F) == 0x01 {
                        is_v2 = true;
                        true
                    } else {
                        false
                    }
                } else if (r1 & 0x04) != 0 {
                    // Illegal command -> legacy SDv1 or MMC
                    is_v2 = false;
                    true
                } else {
                    false
                }
            } else {
                false
            }
        };
        cs.set_high();
        {
            let mut spi_ref = spi.borrow_mut();
            let _ = spi_ref.write(&[0xFF, 0xFF]);
        }

        if !cmd8_ok {
            restore_spi_16m(spi);
            return Err(StorageError::DeviceError);
        }

        // 5. Send ACMD41 loop until card exits idle state (R1 == 0x00)
        let hcs_arg = if is_v2 { 0x40000000 } else { 0x00000000 };
        let mut ready = false;
        for _ in 0..200 {
            // CMD55 (precursor to ACMD)
            cs.set_low();
            {
                let mut spi_ref = spi.borrow_mut();
                send_cmd(&mut spi_ref, 55, 0);
                let _ = wait_r1(&mut spi_ref, 100);
            }
            cs.set_high();
            {
                let mut spi_ref = spi.borrow_mut();
                let _ = spi_ref.write(&[0xFF, 0xFF]);
            }

            // ACMD41
            cs.set_low();
            let r1 = {
                let mut spi_ref = spi.borrow_mut();
                send_cmd(&mut spi_ref, 41, hcs_arg);
                wait_r1(&mut spi_ref, 100)
            };
            cs.set_high();
            {
                let mut spi_ref = spi.borrow_mut();
                let _ = spi_ref.write(&[0xFF, 0xFF]);
            }

            if let Ok(0x00) = r1 {
                ready = true;
                break;
            }
            delay.delay_millis(5);
        }

        if !ready {
            restore_spi_16m(spi);
            return Err(StorageError::DeviceError);
        }

        // 6. Inspect OCR via CMD58 for SDHC/SDXC block-addressing mode
        let mut is_sdhc = false;
        if is_v2 {
            cs.set_low();
            {
                let mut spi_ref = spi.borrow_mut();
                send_cmd(&mut spi_ref, 58, 0);
                if let Ok(0x00) = wait_r1(&mut spi_ref, 100) {
                    let mut ocr = [0xFF; 4];
                    let _ = spi_ref.transfer(&mut ocr);
                    if (ocr[0] & 0x40) != 0 {
                        is_sdhc = true; // CCS bit set -> block addressing
                    }
                }
            }
            cs.set_high();
            {
                let mut spi_ref = spi.borrow_mut();
                let _ = spi_ref.write(&[0xFF, 0xFF]);
            }
        }

        // For standard capacity SDSC cards, force block length to 512 bytes via CMD16
        if !is_sdhc {
            cs.set_low();
            {
                let mut spi_ref = spi.borrow_mut();
                send_cmd(&mut spi_ref, 16, 512);
                let _ = wait_r1(&mut spi_ref, 100);
            }
            cs.set_high();
            {
                let mut spi_ref = spi.borrow_mut();
                let _ = spi_ref.write(&[0xFF, 0xFF]);
            }
        }

        // 7. Transition SPI bus to full 16 MHz operational speed
        let cfg_16m = SpiConfig::default()
            .with_frequency(Rate::from_mhz(16))
            .with_mode(Mode::_0);
        if spi.borrow_mut().apply_config(&cfg_16m).is_err() {
            restore_spi_16m(spi);
            return Err(StorageError::DeviceError);
        }

        // Ensure trailing dummy clocks release MISO to Hi-Z
        cs.set_high();
        {
            let mut spi_ref = spi.borrow_mut();
            let _ = spi_ref.write(&[0xFF, 0xFF]);
        }

        Ok(Self { spi, cs, is_sdhc })
    }
}

impl<'a, 'd> BlockDevice for SpiSdBlockDevice<'a, 'd> {
    fn read_block(&mut self, lba: u32, buf: &mut [u8; SECTOR_SIZE]) -> Result<(), StorageError> {
        let addr = if self.is_sdhc { lba } else { lba.saturating_mul(512) };
        let mut spi = self.spi.borrow_mut();

        // Assert CS
        self.cs.set_low();

        // Wait until card is ready (MISO == 0xFF)
        let mut ready = false;
        for _ in 0..2000 {
            let mut b = [0xFF];
            let _ = spi.transfer(&mut b);
            if b[0] == 0xFF {
                ready = true;
                break;
            }
        }
        if !ready {
            self.cs.set_high();
            let _ = spi.write(&[0xFF, 0xFF]);
            return Err(StorageError::DeviceError);
        }

        // Send CMD17 (READ_SINGLE_BLOCK)
        send_cmd(&mut spi, 17, addr);

        // Expect R1 == 0x00
        match wait_r1(&mut spi, 1000) {
            Ok(0x00) => {}
            _ => {
                self.cs.set_high();
                let _ = spi.write(&[0xFF, 0xFF]);
                return Err(StorageError::DeviceError);
            }
        }

        // Wait for Data Start Token (0xFE) - up to 100,000 polls (~50ms at 16MHz)
        let mut token_found = false;
        for _ in 0..100_000 {
            let mut b = [0xFF];
            let _ = spi.transfer(&mut b);
            if b[0] == 0xFE {
                token_found = true;
                break;
            } else if b[0] != 0xFF {
                // Received Data Error Token (0b0000xxxx)
                break;
            }
        }
        if !token_found {
            self.cs.set_high();
            let _ = spi.write(&[0xFF, 0xFF]);
            return Err(StorageError::DeviceError);
        }

        // Read 512 sector bytes
        buf.fill(0xFF);
        if spi.transfer(buf).is_err() {
            self.cs.set_high();
            let _ = spi.write(&[0xFF, 0xFF]);
            return Err(StorageError::DeviceError);
        }

        // Read 2 CRC bytes
        let mut crc = [0xFF, 0xFF];
        let _ = spi.transfer(&mut crc);

        // Deassert CS and pulse trailing dummy clocks for MISO Hi-Z
        self.cs.set_high();
        let _ = spi.write(&[0xFF, 0xFF]);

        Ok(())
    }

    fn write_block(&mut self, lba: u32, buf: &[u8; SECTOR_SIZE]) -> Result<(), StorageError> {
        let addr = if self.is_sdhc { lba } else { lba.saturating_mul(512) };
        let mut spi = self.spi.borrow_mut();

        // Assert CS
        self.cs.set_low();

        // Wait until card is ready
        let mut ready = false;
        for _ in 0..5000 {
            let mut b = [0xFF];
            let _ = spi.transfer(&mut b);
            if b[0] == 0xFF {
                ready = true;
                break;
            }
        }
        if !ready {
            self.cs.set_high();
            let _ = spi.write(&[0xFF, 0xFF]);
            return Err(StorageError::DeviceError);
        }

        // Send CMD24 (WRITE_BLOCK)
        send_cmd(&mut spi, 24, addr);

        // Expect R1 == 0x00
        match wait_r1(&mut spi, 1000) {
            Ok(0x00) => {}
            _ => {
                self.cs.set_high();
                let _ = spi.write(&[0xFF, 0xFF]);
                return Err(StorageError::DeviceError);
            }
        }

        // Send dummy byte + Data Start Token (0xFE)
        let _ = spi.write(&[0xFF, 0xFE]);

        // Write 512 sector bytes
        if spi.write(buf).is_err() {
            self.cs.set_high();
            let _ = spi.write(&[0xFF, 0xFF]);
            return Err(StorageError::DeviceError);
        }

        // Send 2 dummy CRC bytes
        let _ = spi.write(&[0xFF, 0xFF]);

        // Read Data Response token
        let mut resp = [0xFF];
        let mut got_resp = false;
        for _ in 0..200 {
            let _ = spi.transfer(&mut resp);
            if resp[0] != 0xFF {
                got_resp = true;
                break;
            }
        }
        // 0bxxx0_0101 (0x05) = Data accepted
        if !got_resp || (resp[0] & 0x1F) != 0x05 {
            self.cs.set_high();
            let _ = spi.write(&[0xFF, 0xFF]);
            return Err(StorageError::DeviceError);
        }

        // Busy poll: wait until flash write completes (card holds MISO low 0x00, returns to 0xFF when done)
        // Up to 250,000 polls (~125ms at 16MHz)
        let mut write_finished = false;
        for _ in 0..250_000 {
            let mut b = [0xFF];
            let _ = spi.transfer(&mut b);
            if b[0] == 0xFF {
                write_finished = true;
                break;
            }
        }

        // Deassert CS and pulse trailing dummy clocks for MISO Hi-Z
        self.cs.set_high();
        let _ = spi.write(&[0xFF, 0xFF]);

        if !write_finished {
            return Err(StorageError::DeviceError);
        }

        Ok(())
    }
}
