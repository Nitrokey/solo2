mod store;

use std::path::PathBuf;

use apps::{AdminData, Apps, Dispatch, Variant};
use clap::Parser;
use clap_num::maybe_hex;
use log::info;
use rand_core::{OsRng, RngCore};
use trussed::{
    platform::{consent, reboot, ui},
    types::Location,
    Bytes, Platform,
};
use trussed_usbip::Service;

use store::FilesystemOrRam;

const MANUFACTURER: &str = "Nitrokey";
const PRODUCT: &str = "Nitrokey 3";
const VID: u16 = 0x20a0;
const PID: u16 = 0x42b2;

/// USP/IP based virtualization of a Nitrokey 3 device.
#[derive(Parser, Debug)]
#[clap(about, author, global_setting(clap::AppSettings::NoAutoVersion))]
struct Args {
    /// Print version information.
    #[clap(short, long)]
    version: bool,

    /// Device serial number (default: randomly generated).
    #[clap(short, long, parse(try_from_str=maybe_hex))]
    serial: Option<u128>,

    /// Internal file system (default: use RAM).
    #[clap(short, long)]
    ifs: Option<PathBuf>,

    /// External file system (default: use RAM).
    #[clap(short, long)]
    efs: Option<PathBuf>,
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

struct UserInterface {
    start_time: std::time::Instant,
}

impl UserInterface {
    fn new() -> Self {
        Self {
            start_time: std::time::Instant::now(),
        }
    }
}

impl trussed::platform::UserInterface for UserInterface {
    /// Prompt user to type a word for confirmation
    fn check_user_presence(&mut self) -> consent::Level {
        // use std::io::Read as _;
        // This is not nice - we should "peek" and return Level::None
        // if there is no key pressed yet (unbuffered read from stdin).
        // Couldn't get this to work (without pulling in ncurses or similar).
        // std::io::stdin().bytes().next();
        consent::Level::Normal
    }

    fn set_status(&mut self, status: ui::Status) {
        info!("Set status: {:?}", status);

        if status == ui::Status::WaitingForUserPresence {
            info!(">>>> Received confirmation request. Confirming automatically.");
        }
    }

    fn refresh(&mut self) {}

    fn uptime(&mut self) -> core::time::Duration {
        self.start_time.elapsed()
    }

    fn reboot(&mut self, to: reboot::To) -> ! {
        info!("Restart!  ({:?})", to);
        std::process::exit(25);
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
    type Syscall = Service<FilesystemOrRam, Dispatch>;

    type Reboot = Reboot;

    #[cfg(feature = "provisioner")]
    type Store = store::Store;

    #[cfg(feature = "provisioner")]
    type Filesystem = <store::Store as trussed::store::Store>::I;

    fn uuid(&self) -> [u8; 16] {
        self.serial
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
    exec(store_provider, options, args.serial)
}

fn print_version() {
    let crate_name = clap::crate_name!();
    let crate_version = clap::crate_version!();
    let enabled_features: &[&str] = &[
        #[cfg(feature = "alpha")]
        "alpha",
        #[cfg(feature = "provisioner")]
        "provisioner",
    ];

    print!("{} {}", crate_name, crate_version);
    if !enabled_features.is_empty() {
        print!(" ({})", enabled_features.join(", "));
    }
    println!();
}

fn exec(store: FilesystemOrRam, options: trussed_usbip::Options, serial: Option<u128>) {
    #[cfg(feature = "provisioner")]
    use trussed::virt::StoreProvider as _;

    log::info!("Initializing Trussed");
    trussed_usbip::Builder::new(store, options)
        .dispatch(Dispatch::with_hw_key(
            Location::Internal,
            Bytes::from_slice(b"Unique hw key").unwrap(),
        ))
        .init_platform(move |platform| {
            let ui: Box<dyn trussed::platform::UserInterface + Send + Sync> =
                Box::new(UserInterface::new());
            platform.user_interface().set_inner(ui);
        })
        .build::<Apps<Runner>>()
        .exec(move |_platform| {
            let data = apps::Data {
                admin: AdminData::new(Variant::Usbip),
                #[cfg(feature = "provisioner")]
                provisioner: apps::ProvisionerData {
                    store: unsafe { FilesystemOrRam::store() },
                    stolen_filesystem: unsafe { FilesystemOrRam::ifs() },
                    nfc_powered: false,
                    rebooter: || unimplemented!(),
                },
                _marker: Default::default(),
            };
            (Runner::new(serial), data)
        });
}
