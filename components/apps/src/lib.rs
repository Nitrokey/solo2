#![no_std]

#[cfg(feature = "secrets-app")]
const SECRETS_APP_CREDENTIALS_COUNT_LIMIT: u16 = 50;
#[cfg(feature = "webcrypt")]
const WEBCRYPT_APP_CREDENTIALS_COUNT_LIMIT: u16 = 50;

use apdu_dispatch::{response::SIZE as ApduResponseSize, App as ApduApp};
use bitflags::bitflags;
use core::marker::PhantomData;
use ctaphid_dispatch::{app::App as CtaphidApp, MESSAGE_SIZE as CTAPHID_MESSAGE_SIZE};
#[cfg(feature = "se050")]
use embedded_hal::blocking::delay::DelayUs;
use heapless::Vec;
use littlefs2_core::path;

#[cfg(feature = "factory-reset")]
use admin_app::ResetConfigResult;
use admin_app::{ConfigField, FieldType};

#[macro_use]
extern crate delog;

generate_macros!();

use serde::{Deserialize, Serialize};
#[cfg(all(feature = "opcard", feature = "se050"))]
use trussed::{api::NotBefore, service::Filestore};
use trussed::{
    backend::BackendId,
    client::ClientBuilder,
    interrupt::InterruptFlag,
    platform::Syscall,
    store::filestore::ClientFilestore,
    types::{Location, Mechanism, Path},
    ClientImplementation, Platform, Service,
};

use utils::Version;

pub use admin_app::Reboot;
use admin_app::{ConfigValueMut, ResetSignalAllocation};

#[cfg(feature = "webcrypt")]
use webcrypt::{PeekingBypass, Webcrypt};

mod dispatch;
use dispatch::Backend;
pub use dispatch::Dispatch;

#[cfg(any(feature = "backend-auth", feature = "se050"))]
pub use dispatch::AUTH_LOCATION;

fn is_default<T: Default + PartialEq>(value: &T) -> bool {
    value == &Default::default()
}

mod migrations;

#[derive(Debug, Default, PartialEq, Deserialize, Serialize)]
pub struct Config {
    #[serde(default, rename = "f", skip_serializing_if = "is_default")]
    fido: FidoConfig,
    #[serde(default, rename = "o", skip_serializing_if = "is_default")]
    opcard: OpcardConfig,
    #[cfg(feature = "piv-authenticator")]
    #[serde(default, rename = "p", skip_serializing_if = "is_default")]
    piv: PivConfig,
    #[serde(default, rename = "v", skip_serializing_if = "is_default")]
    fs_version: u32,
    #[cfg(feature = "se050")]
    #[serde(default, rename = "se", skip_serializing_if = "is_default")]
    se050_backend_configured_version: u32,
}

impl admin_app::Config for Config {
    fn field(&mut self, key: &str) -> Option<ConfigValueMut<'_>> {
        let (app, key) = key.split_once('.')?;
        match app {
            "fido" => self.fido.field(key),
            "opcard" => self.opcard.field(key),
            #[cfg(feature = "piv-authenticator")]
            "piv" => self.piv.field(key),
            _ => None,
        }
    }

    fn list_available_fields(&self) -> &'static [ConfigField] {
        &[
            ConfigField {
                name: "fido.disable_skip_up_timeout",
                requires_touch_confirmation: false,
                requires_reboot: false,
                destructive: false,
                ty: FieldType::Bool,
            },
            #[cfg(feature = "se050")]
            ConfigField {
                name: "opcard.use_se050_backend",
                requires_touch_confirmation: true,
                requires_reboot: true,
                destructive: true,
                ty: FieldType::Bool,
            },
            ConfigField {
                name: "opcard.disabled",
                requires_touch_confirmation: false,
                // APDU dispatch does not handle well having the currently select application removed
                requires_reboot: true,
                destructive: false,
                ty: FieldType::Bool,
            },
            #[cfg(feature = "piv-authenticator")]
            ConfigField {
                name: "piv.disabled",
                requires_touch_confirmation: false,
                // APDU dispatch does not handle well having the currently select application removed
                requires_reboot: true,
                destructive: false,
                ty: FieldType::Bool,
            },
        ]
    }

    fn reset_client_id(
        &self,
        key: &str,
    ) -> Option<(&'static Path, &'static ResetSignalAllocation)> {
        #[cfg(feature = "factory-reset")]
        return match (key.split_once('.'), key) {
            (Some(("fido", key)), _) => self.fido.reset_client_id(key),
            (None, "fido") => self.fido.reset_client_id(""),

            (Some(("opcard", key)), _) => self.opcard.reset_client_id(key),
            (None, "opcard") => self.opcard.reset_client_id(""),

            #[cfg(feature = "piv-authenticator")]
            (Some(("piv", key)), _) => self.piv.reset_client_id(key),
            #[cfg(feature = "piv-authenticator")]
            (None, "piv") => self.piv.reset_client_id(""),

            _ => None,
        };

        #[cfg(not(feature = "factory-reset"))]
        {
            _ = key;
            None
        }
    }

    #[cfg(feature = "factory-reset")]
    fn reset_client_config(&mut self, key: &str) -> ResetConfigResult {
        match key {
            "fido" => self.fido.reset_config(),
            "opcard" => self.opcard.reset_config(),
            _ => ResetConfigResult::WrongKey,
        }
    }

    fn migration_version(&self) -> Option<u32> {
        Some(self.fs_version)
    }
    fn set_migration_version(&mut self, version: u32) -> bool {
        self.fs_version = version;
        true
    }
}

#[derive(Debug, Default, PartialEq, Deserialize, Serialize)]
pub struct FidoConfig {
    #[serde(default, rename = "t", skip_serializing_if = "is_default")]
    disable_skip_up_timeout: bool,
}

impl FidoConfig {
    fn field(&mut self, key: &str) -> Option<ConfigValueMut<'_>> {
        match key {
            "disable_skip_up_timeout" => {
                Some(ConfigValueMut::Bool(&mut self.disable_skip_up_timeout))
            }
            _ => None,
        }
    }

    #[cfg(feature = "factory-reset")]
    fn reset_client_id(
        &self,
        _key: &str,
    ) -> Option<(&'static Path, &'static ResetSignalAllocation)> {
        None
    }

    #[cfg(feature = "factory-reset")]
    fn reset_config(&mut self) -> ResetConfigResult {
        use core::mem;
        let old = mem::take(self);

        if &old == self {
            ResetConfigResult::Unchanged
        } else {
            ResetConfigResult::Changed
        }
    }
}

#[derive(Debug, PartialEq, Deserialize, Serialize, Default)]
pub struct OpcardConfig {
    #[cfg(feature = "se050")]
    #[serde(default, rename = "s", skip_serializing_if = "is_default")]
    use_se050_backend: bool,
    #[serde(default, rename = "d", skip_serializing_if = "is_default")]
    disabled: bool,
}

#[cfg(feature = "opcard")]
impl OpcardConfig {
    fn backends(&self) -> &'static [BackendId<Backend>] {
        const BACKENDS_OPCARD_DEFAULT: &[BackendId<Backend>] = &[
            BackendId::Custom(Backend::SoftwareRsa),
            BackendId::Custom(Backend::Auth),
            BackendId::Custom(Backend::Staging),
            BackendId::Core,
        ];
        #[cfg(feature = "se050")]
        const BACKENDS_OPCARD_SE050: &[BackendId<Backend>] = &[
            BackendId::Custom(Backend::Se050),
            BackendId::Custom(Backend::Staging),
            BackendId::Core,
        ];
        #[cfg(feature = "se050")]
        return match self.use_se050_backend {
            true => BACKENDS_OPCARD_SE050,
            false => BACKENDS_OPCARD_DEFAULT,
        };
        #[cfg(not(feature = "se050"))]
        BACKENDS_OPCARD_DEFAULT
    }
}

impl OpcardConfig {
    /// The config value used for initialization and after a factory-reset
    ///
    /// This is distinct from the `Default` value because the old default config was not
    /// enabled
    #[cfg(any(feature = "factory-reset", feature = "se050"))]
    fn init() -> Self {
        Self {
            #[cfg(feature = "se050")]
            use_se050_backend: true,
            disabled: false,
        }
    }

    fn field(&mut self, key: &str) -> Option<ConfigValueMut<'_>> {
        match key {
            #[cfg(feature = "se050")]
            "use_se050_backend" => Some(ConfigValueMut::Bool(&mut self.use_se050_backend)),
            "disabled" => Some(ConfigValueMut::Bool(&mut self.disabled)),
            _ => None,
        }
    }

    #[cfg(feature = "factory-reset")]
    fn reset_client_id(
        &self,
        key: &str,
    ) -> Option<(&'static Path, &'static ResetSignalAllocation)> {
        match key {
            #[cfg(feature = "factory-reset")]
            "" => Some((path!("opcard"), &OPCARD_RESET_SIGNAL)),
            #[cfg(feature = "se050")]
            "use_se050_backend" => Some((path!("opcard"), &OPCARD_RESET_SIGNAL)),
            _ => None,
        }
    }

    #[cfg(feature = "factory-reset")]
    fn reset_config(&mut self) -> ResetConfigResult {
        use core::mem;
        let old = mem::replace(self, Self::init());

        if &old == self {
            ResetConfigResult::Unchanged
        } else {
            ResetConfigResult::Changed
        }
    }
}

#[cfg(feature = "piv-authenticator")]
impl PivConfig {
    fn field(&mut self, key: &str) -> Option<ConfigValueMut<'_>> {
        match key {
            "disabled" => Some(ConfigValueMut::Bool(&mut self.disabled)),
            _ => None,
        }
    }

    #[cfg(feature = "factory-reset")]
    fn reset_client_id(
        &self,
        _key: &str,
    ) -> Option<(&'static Path, &'static ResetSignalAllocation)> {
        None
    }
}

#[cfg(feature = "piv-authenticator")]
#[derive(Debug, PartialEq, Deserialize, Serialize, Default)]
pub struct PivConfig {
    #[serde(default, rename = "d", skip_serializing_if = "is_default")]
    disabled: bool,
}

pub trait Runner {
    type Syscall: Syscall + 'static;

    type Reboot: Reboot;
    type Store: trussed::store::Store;
    #[cfg(feature = "provisioner-app")]
    type Filesystem: trussed::types::LfsStorage + 'static;
    #[cfg(feature = "se050")]
    type Twi: se05x::t1::I2CForT1 + 'static;
    #[cfg(feature = "se050")]
    type Se050Timer: DelayUs<u32> + 'static;
    #[cfg(not(feature = "se050"))]
    type Twi: 'static;
    #[cfg(not(feature = "se050"))]
    type Se050Timer: 'static;

    fn uuid(&self) -> [u8; 16];
    fn is_efs_available(&self) -> bool;
}

pub struct Data<R: Runner> {
    pub admin: AdminData<R>,
    #[cfg(feature = "fido-authenticator")]
    pub fido: FidoData,
    #[cfg(feature = "provisioner-app")]
    pub provisioner: ProvisionerData<R>,
    pub _marker: PhantomData<R>,
}

type Client<R> = ClientImplementation<
    <R as Runner>::Syscall,
    Dispatch<<R as Runner>::Twi, <R as Runner>::Se050Timer>,
>;

type AdminApp<R> = admin_app::App<Client<R>, <R as Runner>::Reboot, AdminStatus, Config>;
#[cfg(feature = "fido-authenticator")]
type FidoApp<R> = fido_authenticator::Authenticator<fido_authenticator::Conforming, Client<R>>;
#[cfg(feature = "ndef-app")]
type NdefApp = ndef_app::App<'static>;
#[cfg(feature = "secrets-app")]
type SecretsApp<R> = secrets_app::Authenticator<Client<R>>;
#[cfg(feature = "webcrypt")]
type WebcryptApp<R> = webcrypt::Webcrypt<Client<R>>;
#[cfg(feature = "opcard")]
type OpcardApp<R> = opcard::Card<Client<R>>;
#[cfg(feature = "piv-authenticator")]
type PivApp<R> = piv_authenticator::Authenticator<Client<R>>;
#[cfg(feature = "provisioner-app")]
type ProvisionerApp<R> =
    provisioner_app::Provisioner<<R as Runner>::Store, <R as Runner>::Filesystem, Client<R>>;

#[repr(u8)]
pub enum CustomStatus {
    ReverseHotpSuccess = 0,
    ReverseHotpError = 1,
}

impl From<CustomStatus> for u8 {
    fn from(status: CustomStatus) -> Self {
        status as _
    }
}

impl TryFrom<u8> for CustomStatus {
    type Error = UnknownStatusError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::ReverseHotpSuccess),
            1 => Ok(Self::ReverseHotpError),
            _ => Err(UnknownStatusError(value)),
        }
    }
}

#[derive(Debug)]
pub struct UnknownStatusError(pub u8);

pub struct Apps<R: Runner> {
    admin: AdminApp<R>,
    #[cfg(all(feature = "fido-authenticator", not(feature = "webcrypt")))]
    fido: Option<FidoApp<R>>,
    #[cfg(feature = "ndef-app")]
    ndef: NdefApp,
    #[cfg(feature = "secrets-app")]
    oath: Option<SecretsApp<R>>,
    #[cfg(feature = "opcard")]
    opcard: Option<OpcardApp<R>>,
    #[cfg(feature = "piv-authenticator")]
    piv: Option<PivApp<R>>,
    #[cfg(feature = "provisioner-app")]
    provisioner: ProvisionerApp<R>,
    #[cfg(feature = "webcrypt")]
    webcrypt: Option<PeekingBypass<'static, FidoApp<R>, WebcryptApp<R>>>,
}

const fn contains(data: &[Mechanism], item: Mechanism) -> bool {
    let mut i = 0;
    while i < data.len() {
        if data[i].const_eq(item) {
            return true;
        }
        i += 1;
    }
    false
}

/// This function ensures that every mechanism that is enabled in trussed-core is implemented by
/// at least one backend (trussed or a custom backend).  It panics if it finds an enabled but
/// unimplemented mechanism.
const fn validate_mechanisms() {
    let enabled = Mechanism::ENABLED;
    let mut i = 0;
    while i < enabled.len() {
        let mechanism = enabled[i];
        i += 1;

        if contains(trussed::types::IMPLEMENTED_MECHANISMS, mechanism) {
            continue;
        }
        #[cfg(feature = "backend-rsa")]
        if contains(trussed_rsa_alloc::MECHANISMS, mechanism) {
            continue;
        }
        #[cfg(feature = "se050")]
        if contains(trussed_se050_backend::MECHANISMS, mechanism) {
            continue;
        }

        // This mechanism is not implemented by Trussed or any of the backends.
        mechanism.panic();
    }
}

impl<R: Runner> Apps<R> {
    pub fn new<P: Platform>(
        runner: &R,
        trussed_service: &mut Service<P, Dispatch<R::Twi, R::Se050Timer>>,
        mut make_client: impl FnMut(
            &mut Service<P, Dispatch<R::Twi, R::Se050Timer>>,
            &'static Path,
            &'static [BackendId<Backend>],
            Option<&'static InterruptFlag>,
        ) -> Client<R>,
        data: Data<R>,
    ) -> Self {
        const {
            validate_mechanisms();
        }

        let _ = (runner, &mut make_client);
        let Data {
            admin,
            #[cfg(feature = "fido-authenticator")]
            fido,
            #[cfg(feature = "provisioner-app")]
            provisioner,
            ..
        } = data;

        let (admin, init_status) =
            Self::admin_app(runner, trussed_service, &mut make_client, admin);

        let mut make_client =
            |ids, backends, interrupt| make_client(trussed_service, ids, backends, interrupt);
        let migrated_successfully = !init_status.contains(InitStatus::MIGRATION_ERROR);
        #[cfg(feature = "opcard")]
        let config_has_error = init_status.contains(InitStatus::CONFIG_ERROR);

        // Config errors can have security and stability implications for opcard as they select
        // the backend to use (se050 or software).  Therefore we disable the app if a config
        // error occured.
        #[cfg(feature = "opcard")]
        let opcard = (!config_has_error && migrated_successfully)
            .then(|| App::new(runner, &mut make_client, (), &admin.config().opcard));
        #[cfg(all(feature = "fido-authenticator", not(feature = "webcrypt")))]
        let fido = migrated_successfully
            .then(|| App::new(runner, &mut make_client, fido, &admin.config().fido));

        #[cfg(feature = "webcrypt")]
        let webcrypt_fido_bypass = migrated_successfully.then(|| {
            PeekingBypass::new(
                App::new(runner, &mut make_client, fido, &admin.config().fido),
                App::new(runner, &mut make_client, (), &()),
            )
        });

        #[cfg(feature = "secrets-app")]
        let oath = migrated_successfully.then(|| App::new(runner, &mut make_client, (), &()));

        #[cfg(feature = "piv-authenticator")]
        let piv = migrated_successfully.then(|| App::new(runner, &mut make_client, (), &()));

        #[cfg(feature = "provisioner-app")]
        let provisioner = App::new(runner, &mut make_client, provisioner, &());

        Self {
            #[cfg(all(feature = "fido-authenticator", not(feature = "webcrypt")))]
            fido,
            #[cfg(feature = "ndef-app")]
            ndef: NdefApp::new(),
            #[cfg(feature = "secrets-app")]
            oath,
            #[cfg(feature = "opcard")]
            opcard,
            #[cfg(feature = "piv-authenticator")]
            piv,
            #[cfg(feature = "provisioner-app")]
            provisioner,
            #[cfg(feature = "webcrypt")]
            webcrypt: webcrypt_fido_bypass,
            admin,
        }
    }

    fn admin_app<P: Platform>(
        runner: &R,
        trussed_service: &mut Service<P, Dispatch<R::Twi, R::Se050Timer>>,
        make_client: impl FnOnce(
            &mut Service<P, Dispatch<R::Twi, R::Se050Timer>>,
            &'static Path,
            &'static [BackendId<Backend>],
            Option<&'static InterruptFlag>,
        ) -> Client<R>,
        mut data: AdminData<R>,
    ) -> (AdminApp<R>, InitStatus) {
        let trussed = AdminApp::<R>::client(
            runner,
            |id, backends, interrupt| make_client(trussed_service, id, backends, interrupt),
            &(),
        );
        // TODO: use CLIENT_ID directly
        let mut filestore = ClientFilestore::new(ADMIN_APP_CLIENT_ID.into(), data.store);
        let version = data.version.encode();

        let valid_migrators = migrations::MIGRATORS;
        // No migrations if the config failed to load. In that case applications are disabled anyways
        let config_error_migrators = &[];

        let mut used_migrators = valid_migrators;

        let mut app = AdminApp::<R>::load_config(
            trussed,
            &mut filestore,
            runner.uuid(),
            version,
            data.version_string,
            data.status(),
            valid_migrators,
        )
        .unwrap_or_else(|(trussed, _err)| {
            data.init_status.insert(InitStatus::CONFIG_ERROR);
            used_migrators = config_error_migrators;
            AdminApp::<R>::with_default_config(
                trussed,
                runner.uuid(),
                version,
                data.version_string,
                data.status(),
                config_error_migrators,
            )
        });

        #[cfg(all(feature = "opcard", feature = "se050"))]
        if !data.init_status.contains(InitStatus::CONFIG_ERROR)
            && app.config().fs_version == 0
            && !app.config().opcard.use_se050_backend
        {
            use core::mem;
            let opcard_trussed_auth_used = trussed_auth::AuthBackend::is_client_active(
                trussed_auth::FilesystemLayout::V0,
                dispatch::AUTH_LOCATION,
                path!("opcard"),
                data.store,
            )
            .unwrap_or_default();
            let mut fs = ClientFilestore::new(path!("opcard").into(), data.store);
            let opcard_used = fs
                .read_dir_first(path!(""), Location::External, &NotBefore::None)
                .unwrap_or_default()
                .is_some();

            if !opcard_trussed_auth_used && !opcard_used {
                // No need to factory reset because the app is not yet created yet
                let mut config = OpcardConfig::init();
                mem::swap(&mut app.config_mut().opcard, &mut config);
                app.save_config_filestore(&mut filestore)
                    .map_err(|_err| {
                        // We reset the config to the old on file version to avoid invalid operations
                        mem::swap(&mut app.config_mut().opcard, &mut config);
                        error_now!("Failed to save config after migration: {_err:?}");
                    })
                    .ok();
            }
        }

        #[cfg(feature = "se050")]
        'se050_configuration: {
            if app.config().se050_backend_configured_version
                != trussed_se050_backend::SE050_CONFIGURE_VERSION
            {
                let Some(se050) = trussed_service.dispatch_mut().se050.as_mut() else {
                    break 'se050_configuration;
                };

                let Ok(_) = se050.configure().map_err(|_err| {
                    error_now!("Failed to configure SE050: {_err:?}");
                    data.init_status.insert(InitStatus::SE050_ERROR);
                    *app.status_mut() = data.status();
                }) else {
                    break 'se050_configuration;
                };

                app.config_mut().se050_backend_configured_version =
                    trussed_se050_backend::SE050_CONFIGURE_VERSION;
                app.save_config_filestore(&mut filestore)
                    .map_err(|_err| {
                        error_now!("Failed to save config after migration: {_err:?}");
                        data.init_status.insert(InitStatus::CONFIG_ERROR);
                        *app.status_mut() = data.status();
                    })
                    .ok();
            }
        }
        let migration_version = used_migrators
            .iter()
            .map(|m| m.version)
            .max()
            .unwrap_or_default();

        let migration_success = app
            .migrate(migration_version, data.store, &mut filestore)
            .is_ok();
        if !migration_success {
            data.init_status.insert(InitStatus::MIGRATION_ERROR);
            *app.status_mut() = data.status();
        }
        (app, data.init_status)
    }

    pub fn with_service<P: Platform>(
        runner: &R,
        trussed_service: &mut Service<P, Dispatch<R::Twi, R::Se050Timer>>,
        data: Data<R>,
    ) -> Self
    where
        R::Syscall: Default,
    {
        Self::new(
            runner,
            trussed_service,
            |trussed_service, id, backends, interrupt| {
                ClientBuilder::new(id)
                    .backends(backends)
                    .interrupt(interrupt)
                    .prepare(trussed_service)
                    .unwrap()
                    .build(R::Syscall::default())
            },
            data,
        )
    }

    pub fn apdu_dispatch<F, T>(&mut self, f: F) -> T
    where
        F: FnOnce(&mut [&mut dyn ApduApp<ApduResponseSize>]) -> T,
    {
        let mut apps: Vec<&mut dyn ApduApp<ApduResponseSize>, 7> = Default::default();

        // App 1: ndef
        #[cfg(feature = "ndef-app")]
        apps.push(&mut self.ndef).ok().unwrap();

        #[cfg(feature = "secrets-app")]
        if let Some(oath) = self.oath.as_mut() {
            apps.push(oath).ok().unwrap();
        }

        #[cfg(feature = "opcard")]
        if let Some(opcard) = self.opcard.as_mut() {
            if !self.admin.config().opcard.disabled {
                apps.push(opcard).ok().unwrap();
            }
        }

        #[cfg(feature = "piv-authenticator")]
        if let Some(piv) = self.piv.as_mut() {
            if !self.admin.config().piv.disabled {
                apps.push(piv).ok().unwrap();
            }
        }

        #[cfg(all(feature = "fido-authenticator", not(feature = "webcrypt")))]
        if let Some(fido) = self.fido.as_mut() {
            apps.push(fido).ok().unwrap();
        }

        // App 6: admin
        apps.push(&mut self.admin).ok().unwrap();

        // App 7: provisioner
        #[cfg(feature = "provisioner-app")]
        apps.push(&mut self.provisioner).ok().unwrap();

        f(&mut apps)
    }

    pub fn ctaphid_dispatch<F, T>(&mut self, f: F) -> T
    where
        F: FnOnce(&mut [&mut dyn CtaphidApp<'static, CTAPHID_MESSAGE_SIZE>]) -> T,
    {
        let mut apps: Vec<&mut dyn CtaphidApp<'static, CTAPHID_MESSAGE_SIZE>, 4> =
            Default::default();

        // App 1: webcrypt or fido
        #[cfg(feature = "webcrypt")]
        if let Some(webcrypt) = self.webcrypt.as_mut() {
            apps.push(webcrypt).ok().unwrap();
        }

        #[cfg(all(feature = "fido-authenticator", not(feature = "webcrypt")))]
        if let Some(fido) = self.fido.as_mut() {
            apps.push(fido).ok().unwrap();
        }

        // App 2: admin
        apps.push(&mut self.admin).ok().unwrap();

        // App 3: secret
        #[cfg(feature = "secrets-app")]
        if let Some(oath) = self.oath.as_mut() {
            apps.push(oath).ok().unwrap();
        }

        // App 4: provisioner
        #[cfg(feature = "provisioner-app")]
        apps.push(&mut self.provisioner).ok().unwrap();

        f(&mut apps)
    }
}

#[cfg(feature = "trussed-usbip")]
impl<R, S> trussed_usbip::Apps<'static, S, Dispatch<R::Twi, R::Se050Timer>> for Apps<R>
where
    R: Runner<Syscall = trussed_usbip::Syscall>,
    S: trussed::virt::StoreProvider,
{
    type Data = (R, Data<R>);

    fn new(
        trussed_service: &mut Service<trussed::virt::Platform<S>, Dispatch<R::Twi, R::Se050Timer>>,
        syscall: trussed_usbip::Syscall,
        (runner, data): (R, Data<R>),
    ) -> Self {
        Self::new(
            &runner,
            trussed_service,
            move |trussed_service, id, backends, _| {
                ClientBuilder::new(id)
                    .backends(backends)
                    .prepare(trussed_service)
                    .unwrap()
                    .build(syscall.clone())
            },
            data,
        )
    }

    fn with_ctaphid_apps<T>(
        &mut self,
        f: impl FnOnce(&mut [&mut dyn CtaphidApp<'static, CTAPHID_MESSAGE_SIZE>]) -> T,
    ) -> T {
        self.ctaphid_dispatch(f)
    }

    #[cfg(feature = "trussed-usbip-ccid")]
    fn with_ccid_apps<T>(
        &mut self,
        f: impl FnOnce(&mut [&mut dyn apdu_dispatch::App<ApduResponseSize>]) -> T,
    ) -> T {
        self.apdu_dispatch(f)
    }
}

trait App<R: Runner>: Sized {
    /// additional data needed by this Trussed app
    type Data;
    type Config;

    /// the desired client ID
    const CLIENT_ID: &'static Path;

    fn new(
        runner: &R,
        make_client: impl FnOnce(
            &'static Path,
            &'static [BackendId<Backend>],
            Option<&'static InterruptFlag>,
        ) -> Client<R>,
        data: Self::Data,
        config: &Self::Config,
    ) -> Self {
        let client = Self::client(runner, make_client, config);
        Self::with_client(runner, client, data, config)
    }

    fn client(
        runner: &R,
        make_client: impl FnOnce(
            &'static Path,
            &'static [BackendId<Backend>],
            Option<&'static InterruptFlag>,
        ) -> Client<R>,
        config: &Self::Config,
    ) -> Client<R> {
        make_client(
            Self::CLIENT_ID,
            Self::backends(runner, config),
            Self::interrupt(),
        )
    }

    fn with_client(runner: &R, trussed: Client<R>, data: Self::Data, config: &Self::Config)
        -> Self;

    fn backends(runner: &R, config: &Self::Config) -> &'static [BackendId<Backend>] {
        let _ = (runner, config);
        const BACKENDS_DEFAULT: &[BackendId<Backend>] = &[];
        BACKENDS_DEFAULT
    }

    fn interrupt() -> Option<&'static InterruptFlag> {
        None
    }
}

#[derive(Copy, Clone)]
pub enum Variant {
    Usbip,
    Lpc55,
    Nrf52,
}

impl From<Variant> for u8 {
    fn from(variant: Variant) -> Self {
        match variant {
            Variant::Usbip => 0,
            Variant::Lpc55 => 1,
            Variant::Nrf52 => 2,
        }
    }
}

bitflags! {
    #[derive(Default, Clone, Copy)]
    pub struct InitStatus: u8 {
        const NFC_ERROR            = 0b00000001;
        const INTERNAL_FLASH_ERROR = 0b00000010;
        const EXTERNAL_FLASH_ERROR = 0b00000100;
        const MIGRATION_ERROR      = 0b00001000;
        const SE050_ERROR          = 0b00010000;
        const CONFIG_ERROR         = 0b00100000;
        const RNG_ERROR            = 0b01000000;
    }
}

pub struct AdminData<R: Runner> {
    pub store: R::Store,
    pub init_status: InitStatus,
    pub ifs_blocks: u8,
    pub efs_blocks: u16,
    pub variant: Variant,
    pub version: Version,
    pub version_string: &'static str,
}

impl<R: Runner> AdminData<R> {
    pub fn new(
        store: R::Store,
        variant: Variant,
        version: Version,
        version_string: &'static str,
    ) -> Self {
        Self {
            store,
            init_status: InitStatus::empty(),
            ifs_blocks: u8::MAX,
            efs_blocks: u16::MAX,
            variant,
            version,
            version_string,
        }
    }
}

pub struct AdminStatus {
    init_status: InitStatus,
    ifs_blocks: u8,
    efs_blocks: u16,
    variant: Variant,
}

impl admin_app::StatusBytes for AdminStatus {
    type Serialized = [u8; 5];
    fn set_random_error(&mut self, value: bool) {
        self.init_status.set(InitStatus::RNG_ERROR, value);
    }
    fn get_random_error(&self) -> bool {
        self.init_status.contains(InitStatus::RNG_ERROR)
    }

    fn serialize(&self) -> [u8; 5] {
        let efs_blocks = self.efs_blocks.to_be_bytes();
        [
            self.init_status.bits(),
            self.ifs_blocks,
            efs_blocks[0],
            efs_blocks[1],
            self.variant.into(),
        ]
    }
}

impl<R: Runner> AdminData<R> {
    fn status(&self) -> AdminStatus {
        AdminStatus {
            init_status: self.init_status,
            ifs_blocks: self.ifs_blocks,
            efs_blocks: self.efs_blocks,
            variant: self.variant,
        }
    }
}

const ADMIN_APP_CLIENT_ID: &Path = path!("admin");

impl<R: Runner> App<R> for AdminApp<R> {
    const CLIENT_ID: &'static Path = ADMIN_APP_CLIENT_ID;

    type Data = AdminData<R>;
    type Config = ();

    fn with_client(runner: &R, trussed: Client<R>, data: Self::Data, _: &()) -> Self {
        let _ = (runner, trussed, data);
        // admin-app is a special case and should only be constructed using Apps::admin_app
        unimplemented!();
    }

    fn interrupt() -> Option<&'static InterruptFlag> {
        static INTERRUPT: InterruptFlag = InterruptFlag::new();
        Some(&INTERRUPT)
    }

    fn backends(runner: &R, _config: &()) -> &'static [BackendId<Backend>] {
        const BACKENDS_ADMIN: &[BackendId<Backend>] = &[
            #[cfg(feature = "se050")]
            BackendId::Custom(Backend::Se050Manage),
            BackendId::Custom(Backend::StagingManage),
            BackendId::Core,
        ];
        let _ = runner;
        BACKENDS_ADMIN
    }
}

#[cfg(feature = "fido-authenticator")]
pub struct FidoData {
    pub has_nfc: bool,
}

#[cfg(feature = "fido-authenticator")]
impl<R: Runner> App<R> for FidoApp<R> {
    const CLIENT_ID: &'static Path = path!("fido");

    type Data = FidoData;
    type Config = FidoConfig;

    fn with_client(runner: &R, trussed: Client<R>, data: FidoData, config: &Self::Config) -> Self {
        let skip_up_timeout = if config.disable_skip_up_timeout {
            None
        } else {
            Some(core::time::Duration::from_secs(2))
        };
        let large_blobs = if cfg!(feature = "nk3-test") && runner.is_efs_available() {
            Some(fido_authenticator::LargeBlobsConfig {
                location: Location::External,
                max_size: 4096,
            })
        } else {
            None
        };
        fido_authenticator::Authenticator::new(
            trussed,
            fido_authenticator::Conforming {},
            fido_authenticator::Config {
                max_msg_size: usbd_ctaphid::constants::MESSAGE_SIZE,
                skip_up_timeout,
                max_resident_credential_count: Some(10),
                large_blobs,
                nfc_transport: data.has_nfc,
            },
        )
    }
    fn interrupt() -> Option<&'static InterruptFlag> {
        static INTERRUPT: InterruptFlag = InterruptFlag::new();
        Some(&INTERRUPT)
    }

    fn backends(_runner: &R, _config: &Self::Config) -> &'static [BackendId<Backend>] {
        &[BackendId::Custom(Backend::Staging), BackendId::Core]
    }
}

#[cfg(feature = "webcrypt")]
impl<R: Runner> App<R> for WebcryptApp<R> {
    const CLIENT_ID: &'static Path = path!("webcrypt");

    type Data = ();
    type Config = ();

    fn with_client(runner: &R, trussed: Client<R>, _: (), _: &()) -> Self {
        let uuid = runner.uuid();
        Webcrypt::new_with_options(
            trussed,
            webcrypt::Options::new(
                Location::External,
                [uuid[0], uuid[1], uuid[2], uuid[3]],
                WEBCRYPT_APP_CREDENTIALS_COUNT_LIMIT,
            ),
        )
    }
    fn backends(runner: &R, _: &()) -> &'static [BackendId<Backend>] {
        const BACKENDS_WEBCRYPT: &[BackendId<Backend>] = &[
            BackendId::Custom(Backend::SoftwareRsa),
            BackendId::Custom(Backend::Staging),
            BackendId::Custom(Backend::Auth),
            BackendId::Core,
        ];
        let _ = runner;
        BACKENDS_WEBCRYPT
    }
}

#[cfg(feature = "secrets-app")]
impl<R: Runner> App<R> for SecretsApp<R> {
    const CLIENT_ID: &'static Path = path!("secrets");

    type Data = ();
    type Config = ();

    fn with_client(runner: &R, trussed: Client<R>, _: (), _: &()) -> Self {
        let uuid = runner.uuid();
        let options = secrets_app::Options::new(
            Location::External,
            CustomStatus::ReverseHotpSuccess.into(),
            CustomStatus::ReverseHotpError.into(),
            [uuid[0], uuid[1], uuid[2], uuid[3]],
            SECRETS_APP_CREDENTIALS_COUNT_LIMIT,
        );
        Self::new(trussed, options)
    }
    fn backends(runner: &R, _: &()) -> &'static [BackendId<Backend>] {
        const BACKENDS_OATH: &[BackendId<Backend>] =
            &[BackendId::Custom(Backend::Auth), BackendId::Core];
        let _ = runner;
        BACKENDS_OATH
    }
    fn interrupt() -> Option<&'static InterruptFlag> {
        static INTERRUPT: InterruptFlag = InterruptFlag::new();
        Some(&INTERRUPT)
    }
}

#[cfg(any(feature = "factory-reset", feature = "se050"))]
static OPCARD_RESET_SIGNAL: ResetSignalAllocation = ResetSignalAllocation::new();

#[cfg(feature = "opcard")]
impl<R: Runner> App<R> for OpcardApp<R> {
    const CLIENT_ID: &'static Path = path!("opcard");

    type Data = ();
    type Config = OpcardConfig;

    fn with_client(runner: &R, trussed: Client<R>, _: (), config: &OpcardConfig) -> Self {
        let _ = config;
        let uuid = runner.uuid();
        let mut options = opcard::Options::default();
        options.button_available = true;
        // See scd/app-openpgp.c in GnuPG for the manufacturer IDs
        options.manufacturer = 0x000Fu16.to_be_bytes();
        options.serial = [uuid[0], uuid[1], uuid[2], uuid[3]];
        options.storage = Location::External;
        {
            use opcard::AllowedAlgorithms as Alg;
            options.allowed_imports = Alg::P_256
                | Alg::RSA_2048
                | Alg::RSA_3072
                | Alg::RSA_4096
                | Alg::X_25519
                | Alg::ED_25519;
            options.allowed_generation = Alg::P_256 | Alg::RSA_2048 | Alg::X_25519 | Alg::ED_25519;
        }
        #[cfg(feature = "se050")]
        {
            if config.use_se050_backend {
                use opcard::AllowedAlgorithms as Alg;
                let algs = [
                    Alg::P_256,
                    Alg::P_384,
                    Alg::P_521,
                    Alg::BRAINPOOL_P256R1,
                    Alg::BRAINPOOL_P384R1,
                    Alg::BRAINPOOL_P512R1,
                    #[cfg(feature = "nk3-test")]
                    Alg::SECP256K1,
                    Alg::RSA_2048,
                    Alg::RSA_3072,
                    Alg::RSA_4096,
                    Alg::X_25519,
                    Alg::ED_25519,
                ]
                .into_iter()
                .fold(Alg::empty(), |acc, v| acc | v);
                options.allowed_imports = algs;
                options.allowed_generation = algs;
            }
        }

        #[cfg(any(feature = "factory-reset", feature = "se050"))]
        {
            options.reset_signal = Some(&OPCARD_RESET_SIGNAL);
        }
        Self::new(trussed, options)
    }
    fn backends(_runner: &R, config: &OpcardConfig) -> &'static [BackendId<Backend>] {
        config.backends()
    }
    fn interrupt() -> Option<&'static InterruptFlag> {
        static INTERRUPT: InterruptFlag = InterruptFlag::new();
        Some(&INTERRUPT)
    }
}

#[cfg(feature = "piv-authenticator")]
impl<R: Runner> App<R> for PivApp<R> {
    const CLIENT_ID: &'static Path = path!("piv");

    type Data = ();
    type Config = ();

    fn with_client(runner: &R, trussed: Client<R>, _: (), _: &()) -> Self {
        Self::new(
            trussed,
            piv_authenticator::Options::default().uuid(Some(runner.uuid())),
        )
    }
    fn backends(runner: &R, _: &()) -> &'static [BackendId<Backend>] {
        const BACKENDS_PIV: &[BackendId<Backend>] = &[
            #[cfg(feature = "se050")]
            BackendId::Custom(Backend::Se050),
            BackendId::Custom(Backend::Staging),
            BackendId::Core,
        ];
        let _ = runner;
        BACKENDS_PIV
    }
    fn interrupt() -> Option<&'static InterruptFlag> {
        static INTERRUPT: InterruptFlag = InterruptFlag::new();
        Some(&INTERRUPT)
    }
}

#[cfg(feature = "provisioner-app")]
pub struct ProvisionerData<R: Runner> {
    pub store: R::Store,
    pub stolen_filesystem: &'static mut R::Filesystem,
    pub nfc_powered: bool,
    pub rebooter: fn() -> !,
}

#[cfg(feature = "provisioner-app")]
impl<R: Runner> App<R> for ProvisionerApp<R> {
    const CLIENT_ID: &'static Path = path!("attn");

    type Data = ProvisionerData<R>;
    type Config = ();

    fn with_client(runner: &R, trussed: Client<R>, data: Self::Data, _: &()) -> Self {
        let uuid = runner.uuid();
        Self::new(
            trussed,
            data.store,
            data.stolen_filesystem,
            data.nfc_powered,
            uuid,
            data.rebooter,
        )
    }
    fn interrupt() -> Option<&'static InterruptFlag> {
        static INTERRUPT: InterruptFlag = InterruptFlag::new();
        Some(&INTERRUPT)
    }
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "piv-authenticator")]
    use super::PivConfig;
    use super::{Config, FidoConfig, OpcardConfig};
    use cbor_smol::cbor_serialize;

    #[test]
    fn test_config_size() {
        let config = Config {
            fido: FidoConfig {
                disable_skip_up_timeout: true,
            },
            opcard: OpcardConfig {
                #[cfg(feature = "se050")]
                use_se050_backend: true,
                disabled: true,
            },
            #[cfg(feature = "piv-authenticator")]
            piv: PivConfig { disabled: true },
            fs_version: 1,
        };
        let mut buffer = [0; 1024];
        let data = cbor_serialize(&config, &mut buffer).unwrap();
        // littlefs2 is most efficient with files < 1/4 of the block size.  The block sizes are 512
        // bytes for LPC55 and 256 bytes for NRF52.  As the block count is only problematic on the
        // LPC55, this could be increased to 128 if necessary.
        assert!(data.len() < 64, "{}: {}", data.len(), hex::encode(data));
    }
}
