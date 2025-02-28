// Based on:
//     https://github.com/lpc55/lpc55-hal/blob/25881c90026b3c7f175ccb5863399c1f18f9836c/src/drivers/flash.rs
// Authors: Conor Patrick, Nicolas Stalder
// License: Apache-2.0 or MIT

use littlefs2::{
    consts::{U512, U8},
    driver::Storage,
    io::{Error, Result},
};
use lpc55_hal::{
    drivers::flash::{FlashGordon, PAGE_SIZE, READ_SIZE, WRITE_SIZE},
    peripherals::prince::{Prince, Region},
    traits::flash::WriteErase,
    typestates::init_state::Enabled,
};

use super::MEMORY_REGIONS;

// The PRINCE peripheral is described in the LPC55S69 user manual (NXP UM11126):
// https://www.mouser.com/pdfDocs/NXP_LPC55S6x_UM.pdf
//
// PRINCE has three regions (§ 48.16.1):
// - region 0: starts at 0x0
// - region 1: starts at 0x40_000
// - region 2: starts at 0x80_000
//
// During provisioning, we set the length of regions 0 and 1 to 0 and the length of region 2 to
// 0x1d_e00.  See utils/lpc55-runner/config/keystore.toml for our configuration and § 7.5.4.2.2
// for a description of the commands.
//
// The bootloader can transparently encrypt and decrypt writes and reads.  This only happens if
// an entire enabled PRINCE region is written or read at once (§ 7.5.4.2.3).  As we always write
// the firmware in one write starting at 0, this can never be the case in our setup.
//
// To use PRINCE from the firmware, it must be enabled first.  Each region is split into
// subregions with a size of 8 kB that can be enabled separately (§ 48.16.2).  If PRINCE is
// enabled, reads from the flash are decrypted transparently.  To encrypt writes, an additional
// ENC_ENABLE register must be set.
//
// For our configuration, this means:
// 1. PRINCE is only used for the filesystem, not for the firmware.
// 2. When accessing the filesystem, we must ensure that we don’t enable decryption for the
//    unencrypted firmware.
// 3. Our filesystem starts at 0x93_000.  As this is not a multiple of the subregion size 8 kB, we
//    need to restrict the firmware area to 0x92_000, the start of the subregion that contains
//    0x93_000.

const BLOCK_SIZE: usize = PAGE_SIZE;
const FLASH_SIZE: usize = 631 * 1024 + 512;
pub const FS_START: usize = MEMORY_REGIONS.filesystem.start;
pub const FS_END: usize = {
    let end = MEMORY_REGIONS.filesystem.end;
    assert!(end <= FLASH_SIZE);
    end
};
pub const BLOCK_COUNT: usize = {
    assert!(FS_START < FS_END);
    assert!(FS_START % BLOCK_SIZE == 0);
    assert!(FS_END % BLOCK_SIZE == 0);
    (FS_END - FS_START) / BLOCK_SIZE
};

const PRINCE_REGION2_START: usize = 0x80_000;
const PRINCE_SUBREGION_SIZE: usize = 8 * 1024;
const PRINCE_REGION2_ENABLE: u32 = {
    // FS must be placed in PRINCE Region 2
    assert!(FS_START >= PRINCE_REGION2_START);
    let offset = FS_START - PRINCE_REGION2_START;
    let subregion_count = offset / PRINCE_SUBREGION_SIZE;

    // Firmware may not overlap with the PRINCE subregions used for the FS
    assert!(
        MEMORY_REGIONS.firmware.end
            <= PRINCE_REGION2_START + subregion_count * PRINCE_SUBREGION_SIZE
    );

    // subregion n is enabled if bit n is set
    // --> disable subregion_count subregions, enable the remaining ones
    0xffffffff << subregion_count
};
const PRINCE_REGION2_DISABLE: u32 = 0;

pub fn enable(prince: &mut Prince<Enabled>) {
    prince.set_region_enable(Region::Region2, PRINCE_REGION2_ENABLE);
}

pub fn disable(prince: &mut Prince<Enabled>) {
    prince.set_region_enable(Region::Region2, PRINCE_REGION2_DISABLE);
}

pub fn with_enabled<T>(prince: &mut Prince<Enabled>, mut f: impl FnMut() -> T) -> T {
    enable(prince);
    let result = f();
    disable(prince);
    result
}

pub struct InternalFilesystem {
    flash_gordon: FlashGordon,
    prince: Prince<Enabled>,
}

impl InternalFilesystem {
    pub fn new(flash_gordon: FlashGordon, prince: Prince<Enabled>) -> Self {
        Self {
            flash_gordon,
            prince,
        }
    }
}

impl Storage for InternalFilesystem {
    const READ_SIZE: usize = READ_SIZE;
    const WRITE_SIZE: usize = WRITE_SIZE;
    const BLOCK_SIZE: usize = BLOCK_SIZE;

    const BLOCK_COUNT: usize = BLOCK_COUNT;
    const BLOCK_CYCLES: isize = -1;

    type CACHE_SIZE = U512;
    type LOOKAHEAD_SIZE = U8;

    fn read(&mut self, off: usize, buf: &mut [u8]) -> Result<usize> {
        with_enabled(&mut self.prince, || {
            let flash: *const u8 = (FS_START + off) as *const u8;
            #[allow(clippy::needless_range_loop)]
            for i in 0..buf.len() {
                buf[i] = unsafe { *flash.add(i) };
            }
        });
        Ok(buf.len())
    }

    fn write(&mut self, off: usize, data: &[u8]) -> Result<usize> {
        let ret = self.prince.write_encrypted(|prince| {
            with_enabled(prince, || self.flash_gordon.write(FS_START + off, data))
        });
        ret.map(|_| data.len()).map_err(|_| Error::IO)
    }

    fn erase(&mut self, off: usize, len: usize) -> Result<usize> {
        assert_eq!(len % BLOCK_SIZE, 0);
        let first_page = (FS_START + off) / BLOCK_SIZE;
        let pages = len / BLOCK_SIZE;
        for i in 0..pages {
            self.flash_gordon
                .erase_page(first_page + i)
                .map_err(|_| Error::IO)?;
        }
        Ok(BLOCK_SIZE * pages)
    }
}
