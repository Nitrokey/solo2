mod store;
mod ui;

use std::{path::PathBuf, sync::Arc, thread};

use apps::{AdminData, Apps, Dispatch, FidoData, Variant};
use clap::{ArgAction, Parser, ValueEnum};
use clap_num::maybe_hex;
use rand_core::{OsRng, RngCore};
use trussed::{types::Location, virt::StoreProvider as _, Bytes, Platform};
use trussed_usbip::Syscall;
use utils::Version;

use store::FilesystemOrRam;
use ui::{Signals, UserInterface, UserPresence};

const VERSION: Version = Version::from_env();
const VERSION_STRING: &str = env!("USBIP_FIRMWARE_VERSION");
const MANUFACTURER: &str = "Nitrokey";
const PRODUCT: &str = "Nitrokey 3";
const VID: u16 = 0x20a0;
const PID: u16 = 0x42b2;

/// USP/IP based virtualization of a Nitrokey 3 device.
#[derive(Parser, Debug)]
#[clap(about, author)]
struct Args {
    /// Print version information.
    #[clap(short, long, action = ArgAction::SetTrue)]
    version: bool,

    /// Device serial number (default: randomly generated).
    #[clap(short, long, value_parser(maybe_hex::<u128>))]
    serial: Option<u128>,

    /// Internal file system (default: use RAM).
    #[clap(short, long)]
    ifs: Option<PathBuf>,

    /// External file system (default: use RAM).
    #[clap(short, long)]
    efs: Option<PathBuf>,

    /// User presence check mechanism.
    ///
    /// The interactive option shows a prompt on stderr requesting consent from the user.  Note
    /// that the runner execution is blocked during the prompt.
    ///
    /// The signal option accepts the next user consent request within one second after a SIGUSR1
    /// signal is received.
    #[clap(short, long, value_enum, default_value_t)]
    user_presence: UserPresenceMechanism,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, ValueEnum)]
enum UserPresenceMechanism {
    #[default]
    AcceptAll,
    RejectAll,
    Interactive,
    Signal,
}

impl From<UserPresenceMechanism> for UserPresence {
    fn from(user_presence: UserPresenceMechanism) -> Self {
        match user_presence {
            UserPresenceMechanism::AcceptAll => Self::Fixed(true),
            UserPresenceMechanism::RejectAll => Self::Fixed(false),
            UserPresenceMechanism::Interactive => Self::Interactive,
            UserPresenceMechanism::Signal => Self::Signal(Arc::new(Signals::new())),
        }
    }
}

struct Reboot;

impl apps::Reboot for Reboot {
    fn reboot() -> ! {
        unimplemented!();
    }

    fn reboot_to_firmware_update() -> ! {
        unimplemented!();
    }

    fn reboot_to_firmware_update_destructive() -> ! {
        unimplemented!();
    }

    fn locked() -> bool {
        false
    }
}

struct Runner {
    serial: [u8; 16],
}

impl Runner {
    fn new(serial: Option<u128>) -> Self {
        let serial = serial.map(u128::to_be_bytes).unwrap_or_else(|| {
            let mut uuid = [0; 16];
            OsRng.fill_bytes(&mut uuid);
            uuid
        });
        Runner { serial }
    }
}

impl apps::Runner for Runner {
    type Syscall = Syscall;

    type Reboot = Reboot;

    type Store = store::Store;

    #[cfg(feature = "provisioner")]
    type Filesystem = store::InternalStorage;

    type Twi = ();
    type Se050Timer = ();

    fn uuid(&self) -> [u8; 16] {
        self.serial
    }

    fn is_efs_available(&self) -> bool {
        true
    }
}

fn main() {
    pretty_env_logger::init();

    let args = Args::parse();
    if args.version {
        print_version();
        return;
    }

    let options = trussed_usbip::Options {
        manufacturer: Some(MANUFACTURER.to_owned()),
        product: Some(PRODUCT.to_owned()),
        serial_number: None,
        vid: VID,
        pid: PID,
    };

    let store_provider = FilesystemOrRam::new(args.ifs, args.efs);
    let user_presence = args.user_presence.into();
    exec(store_provider, options, args.serial, user_presence)
}

fn print_version() {
    let crate_name = clap::crate_name!();
    let crate_version = clap::crate_version!();
    let enabled_features: &[&str] = &[
        #[cfg(feature = "test")]
        "test",
        #[cfg(feature = "provisioner")]
        "provisioner",
    ];

    print!("{} {}", crate_name, crate_version);
    if !enabled_features.is_empty() {
        print!(" ({})", enabled_features.join(", "));
    }
    println!();
}

fn exec(
    store: FilesystemOrRam,
    options: trussed_usbip::Options,
    serial: Option<u128>,
    user_presence: UserPresence,
) {
    if let UserPresence::Signal(signals) = &user_presence {
        let signals = signals.clone();
        thread::spawn(move || {
            signals.update();
        });
    }

    log::info!("Initializing Trussed");
    trussed_usbip::Builder::new(store, options)
        .dispatch(Dispatch::with_hw_key(
            Location::Internal,
            Bytes::from_slice(b"Unique hw key").unwrap(),
        ))
        .init_platform(move |platform| {
            let ui: Box<dyn trussed::platform::UserInterface + Send + Sync> =
                Box::new(UserInterface::new(user_presence.clone()));
            platform.user_interface().set_inner(ui);
        })
        .build::<Apps<Runner>>()
        .exec(move |_platform| {
            let store = unsafe { FilesystemOrRam::store() };
            let data = apps::Data {
                admin: AdminData::new(store, Variant::Usbip, VERSION, VERSION_STRING),
                fido: FidoData { has_nfc: false },
                #[cfg(feature = "provisioner")]
                provisioner: apps::ProvisionerData {
                    store,
                    stolen_filesystem: unsafe { FilesystemOrRam::ifs() },
                    nfc_powered: false,
                    rebooter: || unimplemented!(),
                },
                _marker: Default::default(),
            };
            (Runner::new(serial), data)
        });
}
