use littlefs2::{
    fs::{Allocation, Filesystem},
    io::Result as LfsResult,
};
use memory_regions::MemoryRegions;
use nfc_device::traits::nfc::{Device as NfcDevice, Error as NfcError, State as NfcState};
use nrf52840_hal::{
    gpio::{p0, p1, Level, Output, Pin, PushPull},
    gpiote::Gpiote,
    prelude::OutputPin as _,
    spim,
    timer::Timer,
    twim, Spim, Twim,
};
use nrf52840_pac::{FICR, GPIOTE, P0, P1, POWER, PWM0, PWM1, PWM2, SPIM3, TIMER1, TWIM1};

use crate::{
    flash::ExtFlashStorage,
    soc::nrf52::{flash::FlashStorage, rtic_monotonic::RtcMonotonic, Nrf52},
    ui::UserInterface,
    Board,
};

use migrations::ftl_journal::{self, ifs_flash_old::FlashStorage as OldFlashStorage};
use ui::{HardwareButtons, RgbLed};

pub mod ui;

mod migrations;

type OutPin = Pin<Output<PushPull>>;

const MEMORY_REGIONS: &MemoryRegions = &MemoryRegions::NK3AM;

pub struct NK3AM;

impl Board for NK3AM {
    type Soc = Nrf52;

    type InternalStorage = InternalFlashStorage;
    type ExternalStorage = ExternalFlashStorage;

    type NfcDevice = DummyNfc;
    type Buttons = HardwareButtons;
    type Led = RgbLed;

    #[cfg(feature = "se050")]
    type Twi = Twim<TWIM1>;
    #[cfg(feature = "se050")]
    type Se050Timer = Timer<TIMER1>;
    #[cfg(not(feature = "se050"))]
    type Twi = ();
    #[cfg(not(feature = "se050"))]
    type Se050Timer = ();

    const BOARD_NAME: &'static str = "NK3AM";
    const HAS_NFC: bool = false;

    fn prepare_ifs(ifs: &mut Self::InternalStorage) {
        ifs.format_journal_blocks();
    }

    fn recover_ifs(
        ifs_storage: &mut Self::InternalStorage,
        ifs_alloc: &mut Allocation<Self::InternalStorage>,
        efs_storage: &mut Self::ExternalStorage,
    ) -> LfsResult<()> {
        error_now!("IFS (nrf42) mount-fail");

        // regular mount failed, try mounting "old" (pre-journaling) IFS
        let pac = unsafe { nrf52840_pac::Peripherals::steal() };
        let mut old_ifs_storage = OldFlashStorage::new(pac.NVMC);
        let mut old_ifs_alloc: Allocation<OldFlashStorage> = Filesystem::allocate();
        let old_mountable = Filesystem::is_mountable(&mut old_ifs_storage);

        // we can mount the old ifs filesystem, thus we need to migrate
        if old_mountable {
            let mounted_ifs = ftl_journal::migrate(
                &mut old_ifs_storage,
                &mut old_ifs_alloc,
                ifs_alloc,
                ifs_storage,
                efs_storage,
            );
            // migration went fine => use its resulting IFS
            if let Ok(()) = mounted_ifs {
                info_now!("migration ok, mounting IFS");
                Ok(())
            // migration failed => format IFS
            } else {
                error_now!("failed migration, formatting IFS");
                Filesystem::format(ifs_storage)
            }
        } else {
            info_now!("recovering from journal");
            // IFS and old-IFS cannot be mounted, try to recover from journal
            ifs_storage.recover_from_journal();
            Ok(())
        }
    }
}

pub type InternalFlashStorage =
    FlashStorage<{ MEMORY_REGIONS.filesystem.start }, { MEMORY_REGIONS.filesystem.end }>;
pub type ExternalFlashStorage = ExtFlashStorage<Spim<SPIM3>, OutPin>;

pub struct DummyNfc;

impl NfcDevice for DummyNfc {
    fn read(&mut self, _buf: &mut [u8]) -> Result<NfcState, NfcError> {
        Err(NfcError::NoActivity)
    }
    fn send(&mut self, _buf: &[u8]) -> Result<(), NfcError> {
        Err(NfcError::NoActivity)
    }
    fn frame_size(&self) -> usize {
        0
    }
}

pub struct BoardGPIO {
    pub gpiote: Gpiote,

    /* interactive elements */
    pub rgb_led: [OutPin; 3],
    pub touch: OutPin,

    /* Secure Element (through TWIM1) */
    pub se_pins: Option<twim::Pins>,
    pub se_power: Option<OutPin>,

    /* External Flash & NFC (through SxPIM3) */
    pub flashnfc_spi: Option<spim::Pins>,
    pub flash_cs: Option<OutPin>,
}

pub fn init_pins(gpiote: GPIOTE, p0: P0, p1: P1) -> BoardGPIO {
    let gpiote = Gpiote::new(gpiote);
    let gpio_p0 = p0::Parts::new(p0);
    let gpio_p1 = p1::Parts::new(p1);

    /* touch sensor */
    let touch = gpio_p0.p0_04.into_push_pull_output(Level::High).degrade();
    // not used, just ensure output + low
    gpio_p0.p0_06.into_push_pull_output(Level::Low).degrade();

    /* RGB LED */
    let led_r = gpio_p0.p0_09.into_push_pull_output(Level::Low).degrade();
    let led_g = gpio_p0.p0_10.into_push_pull_output(Level::Low).degrade();
    let led_b = gpio_p1.p1_02.into_push_pull_output(Level::Low).degrade();

    /* SE050 */
    let se_pwr = gpio_p1.p1_10.into_push_pull_output(Level::Low).degrade();
    let se_scl = gpio_p1.p1_15.into_floating_input().degrade();
    let se_sda = gpio_p0.p0_02.into_floating_input().degrade();

    let se_pins = twim::Pins {
        scl: se_scl,
        sda: se_sda,
    };

    /* Ext. Flash SPI */
    // Flash WP# gpio_p0.p0_22
    // Flash HOLD# gpio_p0.p0_23
    let flash_spi_cs = gpio_p0.p0_24.into_push_pull_output(Level::High).degrade();
    let flash_spi_clk = gpio_p1.p1_06.into_push_pull_output(Level::Low).degrade();
    let flash_spi_mosi = gpio_p1.p1_04.into_push_pull_output(Level::Low).degrade();
    let flash_spi_miso = gpio_p1.p1_00.into_floating_input().degrade();
    //let _flash_wp = gpio_p0.p0_22.into_push_pull_output(Level::Low).degrade();
    //let _flash_hold = gpio_p0.p0_23.into_push_pull_output(Level::High).degrade();

    let flash_spi = spim::Pins {
        sck: flash_spi_clk,
        miso: Some(flash_spi_miso),
        mosi: Some(flash_spi_mosi),
    };

    gpiote.reset_events();

    BoardGPIO {
        gpiote,
        rgb_led: [led_r, led_g, led_b],
        touch,
        se_pins: Some(se_pins),
        se_power: Some(se_pwr),
        flashnfc_spi: Some(flash_spi),
        flash_cs: Some(flash_spi_cs),
    }
}

pub fn init_ui(
    leds: [OutPin; 3],
    pwm_red: PWM0,
    pwm_green: PWM1,
    pwm_blue: PWM2,
    touch: OutPin,
) -> UserInterface<RtcMonotonic, HardwareButtons, RgbLed> {
    // TODO: safely share the RTC
    let pac = unsafe { nrf52840_pac::Peripherals::steal() };
    let rtc_mono = RtcMonotonic::new(pac.RTC0);

    let rgb = RgbLed::new(leds, pwm_red, pwm_green, pwm_blue);
    let buttons = HardwareButtons::new(touch);

    UserInterface::new(rtc_mono, Some(buttons), Some(rgb))
}

pub fn init_external_flash(spim3: SPIM3, spi: spim::Pins, cs: OutPin) -> ExternalFlashStorage {
    let spim = Spim::new(spim3, spi, spim::Frequency::M2, spim::MODE_0, 0x00u8);
    ExtFlashStorage::try_new(spim, cs).unwrap()
}

pub fn init_se050(
    twim1: TWIM1,
    pins: twim::Pins,
    mut power: OutPin,
    timer1: TIMER1,
) -> (Twim<TWIM1>, Timer<TIMER1>) {
    power.set_high().unwrap();
    let twim = Twim::new(twim1, pins, twim::Frequency::K400);
    let timer = Timer::new(timer1);
    (twim, timer)
}

pub fn hw_key(ficr: &FICR) -> [u8; 16] {
    let mut er = [0; 16];
    for (i, r) in ficr.er.iter().enumerate() {
        let v = r.read().bits().to_be_bytes();
        for (j, w) in v.into_iter().enumerate() {
            er[i * 4 + j] = w;
        }
    }
    trace!("ER: {:02x?}", er);
    er
}

pub fn power_handler(power: &mut POWER) {
    trace!(
        "irq PWR {:x} {:x} {:x}",
        power.mainregstatus.read().bits(),
        power.usbregstatus.read().bits(),
        power.pofcon.read().bits()
    );

    if power.events_usbdetected.read().events_usbdetected().bits() {
        power.events_usbdetected.write(|w| unsafe { w.bits(0) });
        trace!("usb+");
    }
    if power.events_usbpwrrdy.read().events_usbpwrrdy().bits() {
        power.events_usbpwrrdy.write(|w| unsafe { w.bits(0) });
        trace!("usbY");
    }
    if power.events_usbremoved.read().events_usbremoved().bits() {
        power.events_usbremoved.write(|w| unsafe { w.bits(0) });
        trace!("usb-");
    }
}
