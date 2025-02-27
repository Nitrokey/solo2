// Based on:
//     https://github.com/lpc55/lpc55-hal/blob/25881c90026b3c7f175ccb5863399c1f18f9836c/src/drivers/flash.rs
// Authors: Conor Patrick, Nicolas Stalder
// License: Apache-2.0 or MIT

use littlefs2::{
    driver::Storage,
    io::{Error, Result},
};
use lpc55_hal::{
    drivers::flash::{
        littlefs_params::{self, BLOCK_SIZE},
        FlashGordon,
    },
    peripherals::prince::{Prince, Region},
    traits::flash::WriteErase,
    typestates::init_state::Enabled,
};
use lpc55_pac::PRINCE;

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
const PRINCE_REGION2_MASK: u32 = {
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

pub struct PrinceConfig {
    sr_enable2: u32,
}

impl PrinceConfig {
    pub fn new(prince: &PRINCE) -> Self {
        Self {
            sr_enable2: prince.sr_enable2.read().bits(),
        }
    }

    fn sr_enable2(&self, filesystem: bool) -> u32 {
        if filesystem {
            // enable the default regions and the filesystem regions
            self.sr_enable2 | PRINCE_REGION2_MASK
        } else {
            // enable the default regions without the filesystem regions
            self.sr_enable2 & !PRINCE_REGION2_MASK
        }
    }

    pub fn enable_filesystem(&self, prince: &mut Prince<Enabled>) {
        prince.set_region_enable(Region::Region2, self.sr_enable2(true));
    }

    pub fn disable_filesystem(&self, prince: &mut Prince<Enabled>) {
        prince.set_region_enable(Region::Region2, self.sr_enable2(false));
    }

    pub fn with_filesystem<F: FnMut() -> T, T>(&self, prince: &mut Prince<Enabled>, mut f: F) -> T {
        self.enable_filesystem(prince);
        let result = f();
        self.disable_filesystem(prince);
        result
    }
}

pub struct InternalFilesystem {
    flash_gordon: FlashGordon,
    prince: Prince<Enabled>,
    prince_config: PrinceConfig,
}

impl InternalFilesystem {
    pub fn new(
        flash_gordon: FlashGordon,
        prince: Prince<Enabled>,
        prince_config: PrinceConfig,
    ) -> Self {
        Self {
            flash_gordon,
            prince,
            prince_config,
        }
    }
}

impl Storage for InternalFilesystem {
    const READ_SIZE: usize = littlefs_params::READ_SIZE;
    const WRITE_SIZE: usize = littlefs_params::WRITE_SIZE;
    const BLOCK_SIZE: usize = BLOCK_SIZE;

    const BLOCK_COUNT: usize = BLOCK_COUNT;
    const BLOCK_CYCLES: isize = littlefs_params::BLOCK_CYCLES;

    type CACHE_SIZE = littlefs_params::CACHE_SIZE;
    type LOOKAHEAD_SIZE = littlefs_params::LOOKAHEAD_SIZE;

    fn read(&mut self, off: usize, buf: &mut [u8]) -> Result<usize> {
        self.prince_config.with_filesystem(&mut self.prince, || {
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
            self.prince_config
                .with_filesystem(prince, || self.flash_gordon.write(FS_START + off, data))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_prince_sr_enable2() {
        // currently, the first 9 regions are used for the firmware, so the first 9 bits should
        // stay the same.  the rest should be 0 if the filesystem is disabled or 1 if it is
        // enabled.

        let none = 0;
        let all = u32::MAX;
        let firmware = 0b1_11111111;
        let filesystem = 0b11111111_11111111_11111110_00000000;

        assert_eq!(PRINCE_REGION2_MASK, filesystem);
        assert_eq!(firmware ^ filesystem, all);

        let config = PrinceConfig { sr_enable2: none };
        assert_eq!(config.sr_enable2(false), none);
        assert_eq!(config.sr_enable2(true), filesystem);

        let config = PrinceConfig { sr_enable2: all };
        assert_eq!(config.sr_enable2(false), firmware);
        assert_eq!(config.sr_enable2(true), all);

        let config = PrinceConfig {
            sr_enable2: firmware,
        };
        assert_eq!(config.sr_enable2(false), firmware);
        assert_eq!(config.sr_enable2(true), all);

        let config = PrinceConfig { sr_enable2: 1 };
        assert_eq!(config.sr_enable2(false), 1);
        assert_eq!(
            config.sr_enable2(true),
            0b11111111_11111111_11111110_00000001
        );

        let config = PrinceConfig { sr_enable2: 0x3fff };
        assert_eq!(config.sr_enable2(false), firmware);
        assert_eq!(config.sr_enable2(true), all);
    }
}
