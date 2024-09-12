#![no_std]
#![warn(trivial_casts, unused, unused_qualifications)]

delog::generate_macros!();

use cortex_m_rt::ExceptionFrame;

pub mod flash;
pub mod init;
pub mod runtime;
pub mod soc;
pub mod store;
pub mod ui;

#[cfg(feature = "board-nk3am")]
pub mod nk3am;
#[cfg(feature = "board-nk3xn")]
pub mod nk3xn;
#[cfg(feature = "board-nkpk")]
pub mod nkpk;

use core::marker::PhantomData;

use apps::Dispatch;
#[cfg(feature = "se050")]
use embedded_hal::blocking::delay::DelayUs;
use littlefs2::{
    driver::Storage,
    fs::{Allocation, Filesystem},
    io::Result as LfsResult,
};
use nfc_device::traits::nfc::Device as NfcDevice;
use rand_chacha::ChaCha8Rng;
use trussed::{client::Syscall, Platform};

use crate::{
    soc::{Soc, Uuid},
    store::RunnerStore,
    ui::{buttons::UserPresence, rgb_led::RgbLed, UserInterface},
};

pub type Trussed<B> =
    trussed::Service<RunnerPlatform<B>, Dispatch<<B as Board>::Twi, <B as Board>::Se050Timer>>;
pub type Apps<B> = apps::Apps<Runner<B>>;

pub trait Board {
    type Soc: Soc;

    type InternalStorage: Storage + 'static;
    type ExternalStorage: Storage + 'static;

    type NfcDevice: NfcDevice;
    type Buttons: UserPresence;
    type Led: RgbLed;

    #[cfg(feature = "se050")]
    type Se050Timer: DelayUs<u32> + 'static;
    #[cfg(feature = "se050")]
    type Twi: se05x::t1::I2CForT1 + 'static;
    #[cfg(not(feature = "se050"))]
    type Se050Timer: 'static;
    #[cfg(not(feature = "se050"))]
    type Twi: 'static;

    const BOARD_NAME: &'static str;
    const HAS_NFC: bool;

    fn prepare_ifs(ifs: &mut Self::InternalStorage) {
        let _ = ifs;
    }

    fn recover_ifs(
        ifs_storage: &mut Self::InternalStorage,
        ifs_alloc: &mut Allocation<Self::InternalStorage>,
        efs_storage: &mut Self::ExternalStorage,
    ) -> LfsResult<()> {
        let _ = (ifs_alloc, efs_storage);
        Filesystem::format(ifs_storage)
    }
}

pub struct Runner<B> {
    pub uuid: Uuid,
    pub is_efs_available: bool,
    pub _marker: PhantomData<B>,
}

impl<B: Board> apps::Runner for Runner<B> {
    type Syscall = RunnerSyscall<B::Soc>;
    type Reboot = B::Soc;
    type Store = RunnerStore<B>;
    type Twi = B::Twi;
    type Se050Timer = B::Se050Timer;

    fn uuid(&self) -> [u8; 16] {
        self.uuid
    }

    fn is_efs_available(&self) -> bool {
        self.is_efs_available
    }
}

pub struct RunnerPlatform<B: Board> {
    pub rng: ChaCha8Rng,
    pub store: RunnerStore<B>,
    pub user_interface: UserInterface<<B::Soc as Soc>::Clock, B::Buttons, B::Led>,
}

unsafe impl<B: Board> Platform for RunnerPlatform<B> {
    type R = ChaCha8Rng;
    type S = RunnerStore<B>;
    type UI = UserInterface<<B::Soc as Soc>::Clock, B::Buttons, B::Led>;

    fn user_interface(&mut self) -> &mut Self::UI {
        &mut self.user_interface
    }

    fn rng(&mut self) -> &mut Self::R {
        &mut self.rng
    }

    fn store(&self) -> Self::S {
        self.store
    }
}

pub struct RunnerSyscall<S: Soc> {
    _marker: PhantomData<S>,
}

impl<S: Soc> Default for RunnerSyscall<S> {
    fn default() -> Self {
        Self {
            _marker: Default::default(),
        }
    }
}

impl<S: Soc> Syscall for RunnerSyscall<S> {
    #[inline]
    fn syscall(&mut self) {
        rtic::pend(S::SYSCALL_IRQ);
    }
}

pub fn handle_panic<B: Board>(_info: &core::panic::PanicInfo) -> ! {
    error_now!("{}", _info);
    #[cfg(feature = "rtt-target")]
    rtt_target::rprint!("{}", _info);
    B::Led::set_panic_led();
    loop {
        core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
    }
}

pub fn handle_hard_fault<B: Board>(_ef: &ExceptionFrame) -> ! {
    #[cfg(feature = "rtt-target")]
    rtt_target::rprint!("HardFault: {:?}", _ef);
    B::Led::set_panic_led();
    loop {
        core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
    }
}
