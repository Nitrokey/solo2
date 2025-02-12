#![no_std]
#![warn(trivial_casts, unused, unused_qualifications)]

use apdu_dispatch::interchanges::Responder as CcidResponder;
use boards::{
    init::{UsbNfc, UsbResources},
    soc::Soc,
    Board,
};
use nfc_device::Iso14443;
use usb_device::bus::UsbBusAllocator;
use utils::Version;

delog::generate_macros!();

#[cfg(feature = "board-nk3xn")]
pub mod nk3xn;

#[cfg(not(any(feature = "soc-lpc55", feature = "soc-nrf52")))]
compile_error!("No SoC chosen!");

pub const VERSION: Version = Version::from_env();
pub const VERSION_STRING: &str = env!("NK3_FIRMWARE_VERSION");

#[cfg(feature = "alloc")]
#[global_allocator]
static ALLOCATOR: embedded_alloc::LlffHeap = embedded_alloc::LlffHeap::empty();

#[cfg(feature = "alloc")]
pub fn init_alloc() {
    use core::mem::MaybeUninit;
    const HEAP_SIZE: usize = 32 * 1024;
    static mut HEAP: [MaybeUninit<u8>; HEAP_SIZE] = [MaybeUninit::uninit(); HEAP_SIZE];
    unsafe { ALLOCATOR.init(&raw mut HEAP as usize, HEAP_SIZE) }
}

pub fn init_usb_nfc<B: Board>(
    resources: &'static mut UsbResources<B>,
    usb_bus: Option<UsbBusAllocator<<B::Soc as Soc>::UsbBus>>,
    nfc: Option<Iso14443<B::NfcDevice>>,
    nfc_rp: CcidResponder<'static>,
) -> UsbNfc<B> {
    const USB_PRODUCT: &str = "Nitrokey 3";
    const USB_PRODUCT_ID: u16 = 0x42B2;
    boards::init::init_usb_nfc(
        resources,
        usb_bus,
        nfc,
        nfc_rp,
        USB_PRODUCT,
        USB_PRODUCT_ID,
        VERSION,
    )
}
