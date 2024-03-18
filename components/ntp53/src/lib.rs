#![no_std]

use embedded_hal::blocking::i2c::{Read, Write, WriteRead};
use embedded_hal::digital::v2::InputPin;
use embedded_hal::timer::CountDown;
use embedded_time::duration::{Extensions, Microseconds};

use nfc_device::traits::nfc;

pub mod registers;

use registers::*;

#[macro_use]
extern crate delog;

generate_macros!();

pub const NFC_ADDR: u8 = 0x54;

pub struct Ntp53<I2C, ED> {
    i2c: I2C,
    ed: ED,
    current_frame_size: usize,
}

impl<I2C, ED, E> Ntp53<I2C, ED>
where
    I2C: WriteRead<Error = E> + Write<Error = E> + Read<Error = E>,
    E: core::fmt::Debug,
    ED: InputPin,
{
    pub fn new(i2c: I2C, ed: ED) -> Self {
        Self {
            i2c,
            ed,
            current_frame_size: 0x80,
        }
    }

    pub fn read_status(&mut self) -> Result<StatusRegister, E> {
        let status0 = self.read_register(Register::Status, 0)?.into();
        let status1 = self.read_register(Register::Status, 1)?.into();
        Ok(StatusRegister { status0, status1 })
    }

    pub fn read_config_register(&mut self) -> Result<ConfigRegister, E> {
        let config0 = self.read_register(Register::Config, 0)?.into();
        let config1 = self.read_register(Register::Config, 1)?.into();
        let config2 = self.read_register(Register::Config, 2)?.into();

        Ok(ConfigRegister {
            config0,
            config1,
            config2,
        })
    }

    pub fn release_eeproom_i2c_lock(&mut self) -> Result<(), E> {
        let mut value = StatusRegister1::default();
        value.set_i2c_if_locked(false);
        self.write_register(
            Register::Status,
            1,
            StatusRegister1::I2C_IF_LOCKED_MASK,
            value.into(),
        )
    }

    pub fn read_i2c_slave_config_register(&mut self) -> Result<I2cSlaveConfiguration, E> {
        let addr = self.read_register(Register::I2cSlaveConfig, 0)?;
        let config = self.read_register(Register::I2cSlaveConfig, 1)?;
        Ok(I2cSlaveConfiguration::from_data([addr, config]))
    }

    pub fn read_wdt_register(&mut self) -> Result<WdtRegister, E> {
        let duration_lsb = self.read_register(Register::WdtConfig, 0)?;
        let duration_msb = self.read_register(Register::WdtConfig, 1)?;
        let enable = self.read_wdt_enable()?;
        Ok(WdtRegister {
            duration: u16::from_be_bytes([duration_msb, duration_lsb]),
            enable,
        })
    }

    pub fn read_wdt_enable(&mut self) -> Result<WdtEnableRegister, E> {
        Ok(self.read_register(Register::WdtConfig, 2)?.into())
    }

    pub fn set_wdt_enabled(&mut self, enabled: bool) -> Result<(), E> {
        let mut register = WdtEnableRegister(0);
        register.set_wdt_enable(enabled);
        self.write_register(
            Register::WdtConfig,
            2,
            WdtEnableRegister::WDT_ENABLE_MASK,
            register.into(),
        )
    }

    pub fn read_synch_data_addr(&mut self) -> Result<u16, E> {
        let lsb = self.read_register(Register::SyncDataBlock, 0)?.into();
        let msb = self.read_register(Register::SyncDataBlock, 1)?.into();

        Ok(u16::from_be_bytes([msb, lsb]))
    }

    pub fn write_synch_data_addr(&mut self, addr: u16) -> Result<(), E> {
        let [msb, lsb] = addr.to_be_bytes();
        self.write_register(Register::SyncDataBlock, 0, 0xFF, lsb)?;
        self.write_register(Register::SyncDataBlock, 1, 0xFF, msb)?;

        Ok(())
    }

    pub fn write_block(&mut self, addr: u16, data: [u8; 4]) -> Result<(), E> {
        let [addr_msb, addr_lsb] = addr.to_be_bytes();
        let [b1, b2, b3, b4] = data;
        let buf = [addr_msb, addr_lsb, b1, b2, b3, b4];
        self.i2c.write(NFC_ADDR, &buf)
    }

    pub fn read_block(&mut self, addr: u16) -> Result<[u8; 4], E> {
        let mut data = [0; 4];
        self.i2c
            .write_read(NFC_ADDR, &addr.to_be_bytes(), &mut data)?;
        Ok(data)
    }

    pub fn read_register(&mut self, register: Register, register_offset: u8) -> Result<u8, E> {
        let addr = register as u16;
        let [addr_msb, addr_lsb] = addr.to_be_bytes();
        let mut buffer = [0; 1];
        let data = &[addr_msb, addr_lsb, register_offset];
        self.i2c.write_read(NFC_ADDR, data, &mut buffer)?;
        let [register] = buffer;
        Ok(register)
    }

    pub fn read_register_block(&mut self, register: Register) -> Result<[u8; 4], E> {
        Ok([
            self.read_register(register, 0)?,
            self.read_register(register, 1)?,
            self.read_register(register, 2)?,
            self.read_register(register, 3)?,
        ])
    }

    /// Write `data` to the register
    /// Only the bits set to 1 in `mask` are written
    ///
    /// Registers are 4 bytes, only the byte `offset` is written
    pub fn write_register(
        &mut self,
        register: Register,
        register_offset: u8,
        mask: u8,
        data: u8,
    ) -> Result<(), E> {
        let addr = register as u16;
        let [addr_msb, addr_lsb] = addr.to_be_bytes();
        let data = &[addr_msb, addr_lsb, register_offset, mask, data];
        self.i2c.write(NFC_ADDR, data)
    }

    pub fn test(&mut self, timer: &mut impl CountDown<Time = Microseconds>) {
        match self.read_register_block(Register::Config) {
            Ok(b) => debug_now!("Config register: {:032b}", u32::from_be_bytes(b)),
            Err(_err) => debug_now!("Could not read config block: {_err:?}"),
        };
        let Ok(_config) = self.read_block(0x1037) else {
            error_now!("Could not read config block: ");
            return;
        };
        debug_now!("Release lock: {}", self.release_eeproom_i2c_lock().is_ok());
        debug_now!("Config block: {:032b}", u32::from_be_bytes(_config));

        match self.write_register(Register::Config, 0, 0b0000_0010, 0b0000_0010) {
            Ok(()) => debug_now!("Wrote register"),
            Err(_err) => debug_now!("Failed to write register: {_err:?}"),
        };
        // match self.read_register_block(Register::Config) {
        //     Ok(b) => debug_now!("Config register: {:032b}", u32::from_be_bytes(b)),
        //     Err(_err) => debug_now!("Could not read config register: {_err:?}"),
        // };

        debug_now!("{:?}", self.read_status());
        debug_now!("Synch data addr: {:02X?}", self.read_synch_data_addr());
        debug_now!(
            "Writing data addr: {:?}",
            self.write_synch_data_addr(0x00FF)
        );
        debug_now!("Synch data addr: {:02X?}", self.read_synch_data_addr());
        debug_now!(
            "I2C slave config: {:02X?}",
            self.read_i2c_slave_config_register()
        );
        if let Ok(config) = self.read_config_register() {
            debug_now!("{:?}", config.config0);
            debug_now!("{:?}", config.config1);
            debug_now!("{:?}", config.config2);
        }

        self.set_wdt_enabled(false).ok();

        if let Ok(config) = self.read_config_register() {
            debug_now!("{:?}", config.config0);
            debug_now!("{:?}", config.config1);
            debug_now!("{:?}", config.config2);
        }

        debug_now!("{:?}", self.read_wdt_register());

        let mut pin_inital = (self.ed.is_high().ok(), self.ed.is_low().ok());
        let mut read_synch_data_addr_initial = self.read_synch_data_addr().ok();
        let mut i = 0;
        debug_now!("Status: {:?}", self.read_status());
        loop {
            let pin_data = (self.ed.is_high().ok(), self.ed.is_low().ok());
            if pin_data != pin_inital {
                debug_now!(
                    "Ed PIN: is_high: {:?}, is_low: {:?}",
                    pin_data.0,
                    pin_data.1,
                );
                pin_inital = pin_data;
            }
            let read_synch_data_addr = self.read_synch_data_addr().ok();
            if read_synch_data_addr_initial != read_synch_data_addr {
                debug_now!("Synch data addr: {:02X?}", read_synch_data_addr);
                read_synch_data_addr_initial = read_synch_data_addr;
            }

            if let Ok(i2c_slave_config_register) = self.read_i2c_slave_config_register() {
                if i2c_slave_config_register.config.i2c_wdt_expired() {
                    debug_now!("WDT expired: {:?}", i2c_slave_config_register);
                }
            } else {
                debug_now!("Failed to get I2C slave config");
            }

            timer.start(200_000.microseconds());
            nb::block!(timer.wait()).ok();
            if i % (10_000_000 / 200_000) == 0 {
                debug_now!("Round {i}");
            }

            i += 1;
        }
    }
}

impl<I2C, ED, E> nfc::Device for Ntp53<I2C, ED>
where
    I2C: WriteRead<Error = E> + Write<Error = E> + Read<Error = E>,
    E: core::fmt::Debug,
    ED: InputPin,
{
    fn read(&mut self, buf: &mut [u8]) -> Result<nfc::State, nfc::Error> {
        // self.read_packet(buf)
        todo!()
    }

    fn send(&mut self, buf: &[u8]) -> Result<(), nfc::Error> {
        // self.send_packet(buf)
        todo!()
    }

    fn frame_size(&self) -> usize {
        self.current_frame_size
    }
}
