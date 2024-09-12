#[cfg(feature = "se050")]
use embedded_hal::{blocking::delay::DelayUs, timer::CountDown};
#[cfg(feature = "se050")]
use embedded_time::duration::Microseconds;
#[cfg(feature = "se050")]
use lpc55_hal::drivers::Timer;
use lpc55_hal::{
    drivers::pins::{Pio0_9, Pio1_14},
    peripherals::{ctimer, flexcomm::I2c5},
    typestates::{
        init_state::Unknown,
        pin::{
            function::{FC5_CTS_SDA_SSEL0, FC5_TXD_SCL_MISO_WS},
            state::Special,
        },
    },
    I2cMaster, Pin,
};

use memory_regions::MemoryRegions;
use utils::OptionalStorage;

use crate::{flash::ExtFlashStorage, soc::lpc55::Lpc55, Board};

pub mod button;
pub mod led;
pub mod nfc;
pub mod prince;
pub mod spi;

#[cfg(feature = "no-encrypted-storage")]
use trussed::types::LfsResult;

#[cfg(feature = "no-encrypted-storage")]
lpc55_hal::littlefs2_filesystem!(InternalFilesystem: (prince::FS_START, prince::BLOCK_COUNT));
#[cfg(not(feature = "no-encrypted-storage"))]
use prince::InternalFilesystem;

use nfc::NfcChip;
use spi::{FlashCs, Spi};

pub const MEMORY_REGIONS: &MemoryRegions = &MemoryRegions::NK3XN;

pub type PwmTimer = ctimer::Ctimer3<Unknown>;
pub type ButtonsTimer = ctimer::Ctimer1<Unknown>;

pub type I2C = I2cMaster<
    Pio0_9,
    Pio1_14,
    I2c5,
    (
        Pin<Pio0_9, Special<FC5_TXD_SCL_MISO_WS>>,
        Pin<Pio1_14, Special<FC5_CTS_SDA_SSEL0>>,
    ),
>;

pub struct NK3xN;

impl Board for NK3xN {
    type Soc = Lpc55;

    type InternalStorage = InternalFlashStorage;
    type ExternalStorage = ExternalFlashStorage;

    type NfcDevice = NfcChip;
    type Buttons = button::ThreeButtons;
    type Led = led::RgbLed;

    #[cfg(feature = "se050")]
    type Se050Timer = TimerDelay<Timer<ctimer::Ctimer2<lpc55_hal::Enabled>>>;
    #[cfg(feature = "se050")]
    type Twi = I2C;
    #[cfg(not(feature = "se050"))]
    type Twi = ();
    #[cfg(not(feature = "se050"))]
    type Se050Timer = ();

    const BOARD_NAME: &'static str = "nk3xn";
    const HAS_NFC: bool = true;
}

pub type InternalFlashStorage = InternalFilesystem;
pub type ExternalFlashStorage = OptionalStorage<ExtFlashStorage<Spi, FlashCs>>;

#[cfg(feature = "se050")]
pub struct TimerDelay<T>(pub T);

#[cfg(feature = "se050")]
impl<T> DelayUs<u32> for TimerDelay<T>
where
    T: CountDown<Time = Microseconds<u32>>,
{
    fn delay_us(&mut self, delay: u32) {
        self.0.start(Microseconds::new(delay));
        nb::block!(self.0.wait()).unwrap();
    }
}
