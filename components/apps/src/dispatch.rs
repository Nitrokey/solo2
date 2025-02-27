#[cfg(not(feature = "se050"))]
use core::marker::PhantomData;

use trussed::{
    api::{Reply, Request},
    error::Error as TrussedError,
    service::ServiceResources,
    types::Context,
    Platform,
};

#[cfg(feature = "backend-auth")]
use trussed::types::Location;

use littlefs2_core::{path, Path};

use if_chain::if_chain;
use trussed::{
    api::{reply, request},
    backend::Backend as _,
    serde_extensions::{ExtensionDispatch, ExtensionId, ExtensionImpl},
};

#[cfg(feature = "se050")]
use embedded_hal::blocking::delay::DelayUs;
#[cfg(feature = "se050")]
use se05x::{se05x::Se05X, t1::I2CForT1};
#[cfg(feature = "se050")]
use trussed_se050_backend::{Context as Se050Context, Se050Backend};
#[cfg(feature = "se050")]
use trussed_se050_manage::Se050ManageExtension;

#[cfg(feature = "backend-auth")]
use trussed_auth::AuthExtension;
#[cfg(feature = "backend-auth")]
use trussed_auth_backend::{AuthBackend, AuthContext, MAX_HW_KEY_LEN};

#[cfg(feature = "backend-rsa")]
use trussed_rsa_alloc::SoftwareRsa;

#[cfg(feature = "backend-mldsa")]
use trussed_pqc_backend::SoftwareMldsa;

use trussed_chunked::ChunkedExtension;
use trussed_fs_info::FsInfoExtension;
use trussed_hkdf::HkdfExtension;
use trussed_manage::ManageExtension;
use trussed_staging::{StagingBackend, StagingContext};
use trussed_wrap_key_to_file::WrapKeyToFileExtension;

#[cfg(feature = "backend-auth")]
use super::migrations::TRUSSED_AUTH_FS_LAYOUT;

#[cfg(feature = "se050")]
use super::migrations::SE050_BACKEND_FS_LAYOUT;

#[cfg(feature = "webcrypt")]
use webcrypt::hmacsha256p256::{
    Backend as HmacSha256P256Backend, BackendContext as HmacSha256P256Context,
    HmacSha256P256Extension,
};

#[cfg(feature = "piv-authenticator")]
use trussed_hpke::HpkeExtension;

pub struct Dispatch<T = (), D = ()> {
    #[cfg(feature = "backend-auth")]
    auth: AuthBackend,
    #[cfg(feature = "webcrypt")]
    hmacsha256p256: HmacSha256P256Backend,
    staging: StagingBackend,
    #[cfg(feature = "se050")]
    pub(crate) se050: Option<Se050Backend<T, D>>,
    #[cfg(not(feature = "se050"))]
    __: PhantomData<(T, D)>,
}

#[derive(Default)]
pub struct DispatchContext {
    #[cfg(feature = "backend-auth")]
    auth: AuthContext,
    #[cfg(feature = "webcrypt")]
    hmacsha256p256: HmacSha256P256Context,
    staging: StagingContext,
    #[cfg(feature = "se050")]
    se050: Se050Context,
}

fn should_preserve_file(file: &Path) -> bool {
    // We save all "special" objects, with an ID that is representable by a `u8`

    const DIRS: &[&Path] = &[path!("x5c"), path!("ctr"), path!("sec"), path!("pub")];

    let mut components = file.iter();
    if_chain! {
        if components.next().as_deref() == Some(path!("/"));
        if  components.next().is_some();
        if let Some(intermediary) = components.next();
        if DIRS.contains(&&*intermediary);
        if let Some(file_name) = components.next();
        if components.next().is_none();
        if file_name.as_ref().len() <=2;
        then {
            true
        } else {
            false
        }
    }
}

fn build_staging_backend() -> StagingBackend {
    let mut backend = StagingBackend::new();
    backend.manage.should_preserve_file = |file, _location| should_preserve_file(file);
    backend
}

#[cfg(feature = "se050")]
const NAMESPACE: trussed_se050_backend::namespacing::Namespace = {
    use trussed_se050_backend::namespacing::*;

    Namespace(&[
        NamespaceItem {
            client: path!("admin"),
            value: NamespaceValue::Client1,
        },
        NamespaceItem {
            client: path!("opcard"),
            value: NamespaceValue::Client2,
        },
        NamespaceItem {
            client: path!("piv"),
            value: NamespaceValue::Client3,
        },
    ])
};

#[cfg(any(feature = "backend-auth", feature = "se050"))]
pub const AUTH_LOCATION: Location = Location::Internal;

impl<T: Twi, D: Delay> Dispatch<T, D> {
    #[allow(clippy::new_without_default)]
    pub fn new(
        #[cfg(any(feature = "backend-auth", feature = "se050"))] auth_location: Location,
        #[cfg(feature = "se050")] se050: Option<Se05X<T, D>>,
    ) -> Self {
        Self {
            #[cfg(feature = "backend-auth")]
            auth: AuthBackend::new(auth_location, TRUSSED_AUTH_FS_LAYOUT),
            #[cfg(feature = "webcrypt")]
            hmacsha256p256: Default::default(),
            staging: build_staging_backend(),
            #[cfg(feature = "se050")]
            se050: se050.map(|driver| {
                Se050Backend::new(
                    driver,
                    auth_location,
                    None,
                    NAMESPACE,
                    SE050_BACKEND_FS_LAYOUT,
                )
            }),
            #[cfg(not(feature = "se050"))]
            __: Default::default(),
        }
    }

    #[cfg(feature = "backend-auth")]
    pub fn with_hw_key(
        auth_location: Location,
        hw_key: trussed::Bytes<MAX_HW_KEY_LEN>,
        #[cfg(feature = "se050")] se050: Option<Se05X<T, D>>,
    ) -> Self {
        #[cfg(feature = "se050")]
        // Should the backend really use the same key?
        let hw_key_se050 = hw_key.clone();
        Self {
            auth: AuthBackend::with_hw_key(auth_location, hw_key, TRUSSED_AUTH_FS_LAYOUT),
            #[cfg(feature = "webcrypt")]
            hmacsha256p256: Default::default(),
            staging: build_staging_backend(),
            #[cfg(feature = "se050")]
            se050: se050.map(|driver| {
                Se050Backend::new(
                    driver,
                    auth_location,
                    Some(hw_key_se050),
                    NAMESPACE,
                    SE050_BACKEND_FS_LAYOUT,
                )
            }),
            #[cfg(not(feature = "se050"))]
            __: Default::default(),
        }
    }
}

// HACK around #[cfg] for where clauses. See https://users.rust-lang.org/t/cfg-on-where-clause-items/90292

#[cfg(feature = "se050")]
pub trait Twi: I2CForT1 {}
#[cfg(feature = "se050")]
impl<T: I2CForT1> Twi for T {}
#[cfg(feature = "se050")]
pub trait Delay: DelayUs<u32> {}
#[cfg(feature = "se050")]
impl<D: DelayUs<u32>> Delay for D {}

#[cfg(not(feature = "se050"))]
pub trait Twi {}
#[cfg(not(feature = "se050"))]
impl<T> Twi for T {}

#[cfg(not(feature = "se050"))]
pub trait Delay {}
#[cfg(not(feature = "se050"))]
impl<D> Delay for D {}

impl<T: Twi, D: Delay> ExtensionDispatch for Dispatch<T, D> {
    type Context = DispatchContext;
    type BackendId = Backend;
    type ExtensionId = Extension;

    fn core_request<P: Platform>(
        &mut self,
        backend: &Self::BackendId,
        ctx: &mut Context<Self::Context>,
        request: &Request,
        resources: &mut ServiceResources<P>,
    ) -> Result<Reply, TrussedError> {
        match backend {
            #[cfg(feature = "backend-auth")]
            Backend::Auth => {
                self.auth
                    .request(&mut ctx.core, &mut ctx.backends.auth, request, resources)
            }
            #[cfg(feature = "webcrypt")]
            Backend::HmacSha256P256 => Err(TrussedError::RequestNotAvailable),
            #[cfg(feature = "backend-rsa")]
            Backend::SoftwareRsa => SoftwareRsa.request(&mut ctx.core, &mut (), request, resources),
            #[cfg(feature = "backend-mldsa")]
            Backend::SoftwareMldsa => {
                SoftwareMldsa.request(&mut ctx.core, &mut (), request, resources)
            }
            Backend::Staging => {
                self.staging
                    .request(&mut ctx.core, &mut ctx.backends.staging, request, resources)
            }
            Backend::StagingManage => Err(TrussedError::RequestNotAvailable),
            #[cfg(feature = "se050")]
            Backend::Se050 => self
                .se050
                .as_mut()
                .ok_or(TrussedError::GeneralError)?
                .request(&mut ctx.core, &mut ctx.backends.se050, request, resources),
            #[cfg(feature = "se050")]
            Backend::Se050Manage => Err(TrussedError::RequestNotAvailable),
        }
    }

    fn extension_request<P: Platform>(
        &mut self,
        backend: &Self::BackendId,
        extension: &Self::ExtensionId,
        ctx: &mut Context<Self::Context>,
        request: &request::SerdeExtension,
        resources: &mut ServiceResources<P>,
    ) -> Result<reply::SerdeExtension, TrussedError> {
        #[allow(unreachable_patterns)]
        match backend {
            #[cfg(feature = "backend-auth")]
            Backend::Auth => match extension {
                Extension::Auth => self.auth.extension_request_serialized(
                    &mut ctx.core,
                    &mut ctx.backends.auth,
                    request,
                    resources,
                ),
                #[allow(unreachable_patterns)]
                _ => Err(TrussedError::RequestNotAvailable),
            },
            #[cfg(feature = "webcrypt")]
            Backend::HmacSha256P256 => match extension {
                Extension::HmacSha256P256 => self.hmacsha256p256.extension_request_serialized(
                    &mut ctx.core,
                    &mut ctx.backends.hmacsha256p256,
                    request,
                    resources,
                ),
                _ => Err(TrussedError::RequestNotAvailable),
            },
            #[cfg(feature = "backend-rsa")]
            Backend::SoftwareRsa => Err(TrussedError::RequestNotAvailable),
            #[cfg(feature = "backend-mldsa")]
            Backend::SoftwareMldsa => Err(TrussedError::RequestNotAvailable),
            Backend::Staging => match extension {
                Extension::Chunked => {
                    ExtensionImpl::<ChunkedExtension>::extension_request_serialized(
                        &mut self.staging,
                        &mut ctx.core,
                        &mut ctx.backends.staging,
                        request,
                        resources,
                    )
                }
                Extension::Hkdf => ExtensionImpl::<HkdfExtension>::extension_request_serialized(
                    &mut self.staging,
                    &mut ctx.core,
                    &mut ctx.backends.staging,
                    request,
                    resources,
                ),
                Extension::WrapKeyToFile => {
                    ExtensionImpl::<WrapKeyToFileExtension>::extension_request_serialized(
                        &mut self.staging,
                        &mut ctx.core,
                        &mut ctx.backends.staging,
                        request,
                        resources,
                    )
                }
                Extension::FsInfo => {
                    ExtensionImpl::<FsInfoExtension>::extension_request_serialized(
                        &mut self.staging,
                        &mut ctx.core,
                        &mut ctx.backends.staging,
                        request,
                        resources,
                    )
                }
                #[cfg(feature = "backend-software-hpke")]
                Extension::Hpke => ExtensionImpl::<HpkeExtension>::extension_request_serialized(
                    &mut self.staging,
                    &mut ctx.core,
                    &mut ctx.backends.staging,
                    request,
                    resources,
                ),
                #[allow(unreachable_patterns)]
                _ => Err(TrussedError::RequestNotAvailable),
            },
            Backend::StagingManage => match extension {
                Extension::Manage => {
                    ExtensionImpl::<ManageExtension>::extension_request_serialized(
                        &mut self.staging,
                        &mut ctx.core,
                        &mut ctx.backends.staging,
                        request,
                        resources,
                    )
                }
                _ => Err(TrussedError::RequestNotAvailable),
            },
            #[cfg(feature = "se050")]
            Backend::Se050 => match extension {
                #[cfg(feature = "trussed-auth")]
                Extension::Auth => ExtensionImpl::<AuthExtension>::extension_request_serialized(
                    self.se050.as_mut().ok_or(TrussedError::GeneralError)?,
                    &mut ctx.core,
                    &mut ctx.backends.se050,
                    request,
                    resources,
                ),
                Extension::WrapKeyToFile => {
                    ExtensionImpl::<WrapKeyToFileExtension>::extension_request_serialized(
                        self.se050.as_mut().ok_or(TrussedError::GeneralError)?,
                        &mut ctx.core,
                        &mut ctx.backends.se050,
                        request,
                        resources,
                    )
                }
                #[cfg(feature = "piv-authenticator")]
                Extension::Hpke => ExtensionImpl::<HpkeExtension>::extension_request_serialized(
                    self.se050.as_mut().ok_or(TrussedError::GeneralError)?,
                    &mut ctx.core,
                    &mut ctx.backends.se050,
                    request,
                    resources,
                ),
                _ => Err(TrussedError::RequestNotAvailable),
            },
            #[cfg(feature = "se050")]
            Backend::Se050Manage => match extension {
                Extension::Manage => {
                    ExtensionImpl::<ManageExtension>::extension_request_serialized(
                        self.se050.as_mut().ok_or(TrussedError::GeneralError)?,
                        &mut ctx.core,
                        &mut ctx.backends.se050,
                        request,
                        resources,
                    )
                }
                Extension::Se050Manage => {
                    ExtensionImpl::<Se050ManageExtension>::extension_request_serialized(
                        self.se050.as_mut().ok_or(TrussedError::GeneralError)?,
                        &mut ctx.core,
                        &mut ctx.backends.se050,
                        request,
                        resources,
                    )
                }
                _ => Err(TrussedError::RequestNotAvailable),
            },
            _ => Err(TrussedError::RequestNotAvailable),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Backend {
    #[cfg(feature = "backend-auth")]
    Auth,
    #[cfg(feature = "webcrypt")]
    HmacSha256P256,
    #[cfg(feature = "backend-rsa")]
    SoftwareRsa,
    #[cfg(feature = "backend-mldsa")]
    SoftwareMldsa,
    Staging,
    /// Separate BackendId to prevent non-priviledged apps from accessing the manage Extension
    StagingManage,
    #[cfg(feature = "se050")]
    Se050,
    #[cfg(feature = "se050")]
    /// Separate BackendId to prevent non-priviledged apps from accessing the manage Extension
    Se050Manage,
}

#[derive(Debug, Clone, Copy)]
pub enum Extension {
    #[cfg(feature = "backend-auth")]
    Auth,
    Hkdf,
    Chunked,
    WrapKeyToFile,
    Manage,
    FsInfo,
    #[cfg(feature = "webcrypt")]
    HmacSha256P256,
    #[cfg(feature = "se050")]
    Se050Manage,
    #[cfg(feature = "piv-authenticator")]
    Hpke,
}

impl From<Extension> for u8 {
    fn from(extension: Extension) -> Self {
        match extension {
            #[cfg(feature = "backend-auth")]
            Extension::Auth => 0,
            Extension::Chunked => 1,
            Extension::WrapKeyToFile => 2,
            Extension::Manage => 3,
            #[cfg(feature = "webcrypt")]
            Extension::HmacSha256P256 => 4,
            #[cfg(feature = "se050")]
            Extension::Se050Manage => 5,
            Extension::Hkdf => 6,
            Extension::FsInfo => 7,
            #[cfg(feature = "piv-authenticator")]
            Extension::Hpke => 8,
        }
    }
}

impl TryFrom<u8> for Extension {
    type Error = TrussedError;

    fn try_from(id: u8) -> Result<Self, Self::Error> {
        match id {
            #[cfg(feature = "backend-auth")]
            0 => Ok(Extension::Auth),
            1 => Ok(Extension::Chunked),
            2 => Ok(Extension::WrapKeyToFile),
            3 => Ok(Extension::Manage),
            #[cfg(feature = "webcrypt")]
            4 => Ok(Extension::HmacSha256P256),
            #[cfg(feature = "se050")]
            5 => Ok(Extension::Se050Manage),
            6 => Ok(Extension::Hkdf),
            7 => Ok(Extension::FsInfo),
            #[cfg(feature = "piv-authenticator")]
            8 => Ok(Extension::Hpke),
            _ => Err(TrussedError::InternalError),
        }
    }
}

#[cfg(feature = "backend-auth")]
impl<T: Twi, D: Delay> ExtensionId<AuthExtension> for Dispatch<T, D> {
    type Id = Extension;

    const ID: Self::Id = Self::Id::Auth;
}

impl<T: Twi, D: Delay> ExtensionId<HkdfExtension> for Dispatch<T, D> {
    type Id = Extension;

    const ID: Self::Id = Self::Id::Hkdf;
}

impl<T: Twi, D: Delay> ExtensionId<ChunkedExtension> for Dispatch<T, D> {
    type Id = Extension;

    const ID: Self::Id = Self::Id::Chunked;
}

impl<T: Twi, D: Delay> ExtensionId<WrapKeyToFileExtension> for Dispatch<T, D> {
    type Id = Extension;

    const ID: Self::Id = Self::Id::WrapKeyToFile;
}

#[cfg(feature = "webcrypt")]
impl<T: Twi, D: Delay> ExtensionId<HmacSha256P256Extension> for Dispatch<T, D> {
    type Id = Extension;

    const ID: Self::Id = Self::Id::HmacSha256P256;
}

impl<T: Twi, D: Delay> ExtensionId<ManageExtension> for Dispatch<T, D> {
    type Id = Extension;

    const ID: Self::Id = Self::Id::Manage;
}

#[cfg(feature = "se050")]
impl<T: Twi, D: Delay> ExtensionId<Se050ManageExtension> for Dispatch<T, D> {
    type Id = Extension;

    const ID: Self::Id = Self::Id::Se050Manage;
}

impl<T: Twi, D: Delay> ExtensionId<FsInfoExtension> for Dispatch<T, D> {
    type Id = Extension;

    const ID: Self::Id = Self::Id::FsInfo;
}

#[cfg(feature = "piv-authenticator")]
impl<T: Twi, D: Delay> ExtensionId<HpkeExtension> for Dispatch<T, D> {
    type Id = Extension;

    const ID: Self::Id = Self::Id::Hpke;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_preserve() {
        assert!(should_preserve_file(path!("/fido/sec/00")));
        assert!(should_preserve_file(path!("/fido/x5c/00")));
        assert!(should_preserve_file(path!("/fido/sec/01")));
        assert!(should_preserve_file(path!("/fido/x5c/01")));
        assert!(should_preserve_file(path!("/attn/pub/00")));
        assert!(should_preserve_file(path!("/attn/sec/01")));
        assert!(should_preserve_file(path!("/attn/sec/02")));
        assert!(should_preserve_file(path!("/attn/sec/03")));
        assert!(should_preserve_file(path!("/attn/x5c/01")));
        assert!(should_preserve_file(path!("/attn/x5c/02")));
        assert!(should_preserve_file(path!("/attn/x5c/03")));
        assert!(!should_preserve_file(path!("/fido/dat/sec/00")));
    }
}
