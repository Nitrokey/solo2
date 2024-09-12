use littlefs2::{fs::Allocation, io::Result as LfsResult};
use memory_regions::MemoryRegions;
use utils::RamStorage;

use super::nk3am::{
    self,
    ui::{HardwareButtons, RgbLed},
    DummyNfc,
};
use crate::{
    soc::nrf52::{flash::FlashStorage, Nrf52},
    Board,
};

pub use nk3am::{init_pins, init_ui, power_handler};

const MEMORY_REGIONS: &MemoryRegions = &MemoryRegions::NKPK;

pub struct NKPK;

impl Board for NKPK {
    type Soc = Nrf52;

    type InternalStorage = InternalFlashStorage;
    type ExternalStorage = ExternalFlashStorage;

    type NfcDevice = DummyNfc;
    type Buttons = HardwareButtons;
    type Led = RgbLed;

    type Twi = ();
    type Se050Timer = ();

    const BOARD_NAME: &'static str = "NKPK";
    const HAS_NFC: bool = false;

    fn prepare_ifs(ifs: &mut Self::InternalStorage) {
        ifs.format_journal_blocks();
    }

    fn recover_ifs(
        ifs_storage: &mut Self::InternalStorage,
        _ifs_alloc: &mut Allocation<Self::InternalStorage>,
        _efs_storage: &mut Self::ExternalStorage,
    ) -> LfsResult<()> {
        error_now!("IFS (nrf42) mount-fail");
        // IFS cannot be mounted, try to recover from journal
        ifs_storage.recover_from_journal();
        Ok(())
    }
}

pub type InternalFlashStorage =
    FlashStorage<{ MEMORY_REGIONS.filesystem.start }, { MEMORY_REGIONS.filesystem.end }>;
// TODO: Do we want to mirror the NK3AM EFS?
pub type ExternalFlashStorage = RamStorage<nk3am::ExternalFlashStorage, 256>;
