//! Hardware Abstraction Layer for LilyGO T-Dongle-C5

pub mod pinout {
    // LCD ST7735 Pins
    pub const LCD_BL_GPIO: u8 = 0;
    pub const LCD_RST_GPIO: u8 = 1;
    pub const LCD_MOSI_GPIO: u8 = 2;
    pub const LCD_DC_GPIO: u8 = 3;
    pub const LCD_SCK_GPIO: u8 = 6;
    pub const LCD_CS_GPIO: u8 = 10;

    // MicroSD SPI Chip Select
    pub const SD_CS_GPIO: u8 = 23;

    // APA102 DotStar RGB LED Pins
    pub const LED_CLK_GPIO: u8 = 4;
    pub const LED_DATA_GPIO: u8 = 5;

    // BOOT Button (Active-Low)
    pub const BOOT_BTN_GPIO: u8 = 28;
}
