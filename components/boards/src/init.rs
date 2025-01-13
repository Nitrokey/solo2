use apdu_dispatch::{
    dispatch::ApduDispatch,
    interchanges::{Channel as CcidChannel, Responder as CcidResponder, SIZE as CCID_SIZE},
};
#[cfg(any(feature = "trussed-auth", feature = "se050"))]
use apps::AUTH_LOCATION;
use apps::{AdminData, Data, Dispatch, FidoData, InitStatus};

use ctaphid_dispatch::{Channel as CtapChannel, Dispatch as CtaphidDispatch};
#[cfg(not(feature = "no-delog"))]
use delog::delog;
use interchange::Channel;
use nfc_device::Iso14443;
use rand::{CryptoRng, Rng as _, RngCore, SeedableRng};
use rand_chacha::ChaCha8Rng;
use ref_swap::OptionRefSwap;
use trussed::{interrupt::InterruptFlag, platform::Store as _};
use usb_device::{
    bus::UsbBusAllocator,
    device::{UsbDevice, UsbDeviceBuilder, UsbVidPid},
};
use usbd_ccid::Ccid;
use usbd_ctaphid::CtapHid;
use utils::Version;

use crate::{soc::Soc, Apps, Board, Runner, RunnerPlatform, RunnerStore, Trussed, UserInterface};

#[cfg(not(feature = "no-delog"))]
delog!(Delogger, 3 * 1024, 512, DelogFlusher);

#[derive(Debug)]
pub struct DelogFlusher {}

impl delog::Flusher for DelogFlusher {
    fn flush(&self, _msg: &str) {
        #[cfg(feature = "log-rtt")]
        rtt_target::rprint!(_msg);

        #[cfg(feature = "log-semihosting")]
        cortex_m_semihosting::hprint!(_msg).ok();
    }
}

pub static DELOG_FLUSHER: DelogFlusher = DelogFlusher {};

pub fn init_logger<B: Board>(_version: &str) {
    #[cfg(feature = "log-rtt")]
    rtt_target::rtt_init_print!();

    #[cfg(not(feature = "no-delog"))]
    Delogger::init_default(delog::LevelFilter::Debug, &DELOG_FLUSHER).ok();

    info!(
        "Embedded Runner ({}:{}) using librunner {}",
        B::Soc::SOC_NAME,
        B::BOARD_NAME,
        _version,
    );
}

pub struct UsbClasses<S: Soc> {
    pub usbd: UsbDevice<'static, S::UsbBus>,
    pub ccid: Ccid<'static, 'static, S::UsbBus, CCID_SIZE>,
    pub ctaphid: CtapHid<'static, 'static, 'static, S::UsbBus>,
}

impl<S: Soc> UsbClasses<S> {
    pub fn poll(&mut self) {
        self.ctaphid.check_for_app_response();
        self.ccid.check_for_app_response();
        self.usbd.poll(&mut [&mut self.ccid, &mut self.ctaphid]);
    }
}

pub struct UsbNfc<B: Board> {
    pub usb_classes: Option<UsbClasses<B::Soc>>,
    pub apdu_dispatch: ApduDispatch<'static>,
    pub ctaphid_dispatch: CtaphidDispatch<'static, 'static>,
    pub iso14443: Option<Iso14443<B::NfcDevice>>,
}

const CARD_ISSUER: &[u8; 13] = b"Nitrokey\0\0\0\0\0";
const USB_MANUFACTURER: &str = "Nitrokey";
const USB_VENDOR_ID: u16 = 0x20A0;

pub fn init_usb_nfc<B: Board>(
    usb_bus: Option<&'static UsbBusAllocator<<B::Soc as Soc>::UsbBus>>,
    nfc: Option<Iso14443<B::NfcDevice>>,
    nfc_rp: CcidResponder<'static>,
    usb_product: &'static str,
    usb_product_id: u16,
    version: Version,
) -> UsbNfc<B> {
    static CCID_CHANNEL: CcidChannel = Channel::new();
    static CTAP_CHANNEL: CtapChannel = Channel::new();
    static CTAP_INTERRUPT: OptionRefSwap<'static, InterruptFlag> = OptionRefSwap::new(None);

    /* claim interchanges */
    let (ccid_rq, ccid_rp) = CCID_CHANNEL.split().unwrap();
    let (ctaphid_rq, ctaphid_rp) = CTAP_CHANNEL.split().unwrap();

    /* initialize dispatchers */
    let apdu_dispatch = ApduDispatch::new(ccid_rp, nfc_rp);
    let ctaphid_dispatch = CtaphidDispatch::with_interrupt(ctaphid_rp, Some(&CTAP_INTERRUPT));

    /* populate requesters (if bus options are provided) */
    let usb_classes = usb_bus.map(|usb_bus| {
        /* Class #1: CCID */
        let ccid = Ccid::new(usb_bus, ccid_rq, Some(CARD_ISSUER));

        /* Class #2: CTAPHID */
        let ctaphid = CtapHid::with_interrupt(usb_bus, ctaphid_rq, Some(&CTAP_INTERRUPT), 0u32)
            .implements_ctap1()
            .implements_ctap2()
            .implements_wink();

        let vidpid = UsbVidPid(USB_VENDOR_ID, usb_product_id);
        let usbd = UsbDeviceBuilder::new(usb_bus, vidpid)
            .product(usb_product)
            .manufacturer(USB_MANUFACTURER)
            .device_release(version.usb_release())
            .max_packet_size_0(64)
            .composite_with_iads()
            .build();

        UsbClasses {
            usbd,
            ccid,
            ctaphid,
        }
    });

    UsbNfc {
        usb_classes,
        apdu_dispatch,
        ctaphid_dispatch,
        iso14443: nfc,
    }
}

pub fn init_apps<B: Board>(
    soc: &B::Soc,
    trussed: &mut Trussed<B>,
    init_status: InitStatus,
    store: &RunnerStore<B>,
    nfc_powered: bool,
    version: Version,
    version_string: &'static str,
) -> Apps<B> {
    let mut admin = AdminData::new(*store, B::Soc::VARIANT, version, version_string);
    admin.init_status = init_status;
    if !nfc_powered {
        if let Ok(ifs_blocks) = store.ifs().available_blocks() {
            if let Ok(ifs_blocks) = u8::try_from(ifs_blocks) {
                admin.ifs_blocks = ifs_blocks;
            }
        }
        if let Ok(efs_blocks) = store.efs().available_blocks() {
            if let Ok(efs_blocks) = u16::try_from(efs_blocks) {
                admin.efs_blocks = efs_blocks;
            }
        }
    }

    #[cfg(feature = "provisioner")]
    let provisioner = {
        use apps::Reboot as _;
        let store = store.clone();
        let int_flash_ref = unsafe { crate::store::steal_internal_storage::<B>() };
        let rebooter: fn() -> ! = B::Soc::reboot_to_firmware_update;

        apps::ProvisionerData {
            store,
            stolen_filesystem: int_flash_ref,
            nfc_powered,
            rebooter,
        }
    };

    let runner = Runner {
        uuid: *soc.uuid(),
        is_efs_available: !nfc_powered,
        _marker: Default::default(),
    };
    let data = Data {
        admin,
        fido: FidoData {
            has_nfc: B::HAS_NFC,
        },
        #[cfg(feature = "provisioner")]
        provisioner,
        _marker: Default::default(),
    };
    Apps::with_service(&runner, trussed, data)
}

#[cfg(feature = "se050")]
fn init_se050<
    I2C: se05x::t1::I2CForT1,
    D: embedded_hal::blocking::delay::DelayUs<u32>,
    R: CryptoRng + RngCore,
>(
    i2c: I2C,
    delay: D,
    dev_rng: &mut R,
    init_status: &mut InitStatus,
) -> (se05x::se05x::Se05X<I2C, D>, [u8; 32]) {
    use se05x::se05x::commands::GetRandom;

    let seed: [u8; 32] = dev_rng.gen();
    let mut se050 = se05x::se05x::Se05X::new(i2c, 0x48, delay);
    let seed = (|| {
        se050.enable()?;
        let buf = &mut [0; 100];
        let se050_rand = se050.run_command(&GetRandom { length: 32.into() }, buf)?;
        let mut s: [u8; 32] = se050_rand
            .data
            .try_into()
            .or(Err(se05x::se05x::Error::Unknown))?;
        for (se050, orig) in s.iter_mut().zip(seed) {
            *se050 ^= orig;
        }
        Ok::<_, se05x::se05x::Error>(s)
    })()
    .unwrap_or_else(|_err| {
        debug_now!("Got error when getting SE050 initial entropy: {_err:?}");
        *init_status |= InitStatus::SE050_ERROR;
        seed
    });
    (se050, seed)
}

pub fn init_trussed<B: Board, R: CryptoRng + RngCore>(
    dev_rng: &mut R,
    store: RunnerStore<B>,
    user_interface: UserInterface<<B::Soc as Soc>::Clock, B::Buttons, B::Led>,
    init_status: &mut InitStatus,
    #[cfg(feature = "trussed-auth")] hw_key: Option<&[u8]>,
    #[cfg(feature = "se050")] se050: Option<(B::Twi, B::Se050Timer)>,
) -> Trussed<B> {
    #[cfg(feature = "se050")]
    let (se050, seed) = if let Some((twi, timer)) = se050 {
        let (se050, seed) = init_se050(twi, timer, dev_rng, init_status);
        (Some(se050), Some(seed))
    } else {
        (None, None)
    };
    #[cfg(not(feature = "se050"))]
    let seed = None;

    // False positive due to cfg
    #[allow(clippy::unnecessary_literal_unwrap)]
    let rng = ChaCha8Rng::from_seed(seed.unwrap_or_else(|| dev_rng.gen()));
    let _ = init_status;

    let platform = RunnerPlatform {
        rng,
        store,
        user_interface,
    };

    #[cfg(feature = "trussed-auth")]
    let dispatch = if let Some(hw_key) = hw_key {
        Dispatch::with_hw_key(
            AUTH_LOCATION,
            trussed::types::Bytes::from_slice(hw_key).unwrap(),
            #[cfg(feature = "se050")]
            se050,
        )
    } else {
        Dispatch::new(
            AUTH_LOCATION,
            #[cfg(feature = "se050")]
            se050,
        )
    };
    #[cfg(not(feature = "trussed-auth"))]
    let dispatch = Dispatch::new(
        #[cfg(feature = "se050")]
        AUTH_LOCATION,
        #[cfg(feature = "se050")]
        se050,
    );

    Trussed::with_dispatch(platform, dispatch)
}
