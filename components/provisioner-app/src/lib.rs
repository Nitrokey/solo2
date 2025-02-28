//! # Solo 2 provisioner app
//!
//! This is a highly *non-portable* Trussed app.
//!
//! It allows injecting arbitrary binary files at arbitrary paths, e.g., to inject FIDO batch
//! attestation keys.
//! It allows generating Trussed device attestation keys and obtaining their public keys,
//! to then generate and inject attn certs from a given root or intermedidate CA.
//!
//! See `solo2-cli` for usage.
#![no_std]

mod apdu;
mod ctaphid;

#[macro_use]
extern crate delog;
generate_macros!();

use core::convert::TryFrom;
use heapless::Vec;
use littlefs2::{
    path,
    path::{Path, PathBuf},
};
use trussed::{
    client,
    key::{Flags, Key, Kind as KeyKind},
    store::{self, Store},
    syscall,
    types::LfsStorage,
};

const TESTER_FILENAME_ID: [u8; 2] = [0xe1, 0x01];
const TESTER_FILE_ID: [u8; 2] = [0xe1, 0x02];

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum Instruction {
    Select,
    WriteBinary,

    WriteFile,

    BootToBootrom,
    ReformatFilesystem,
    GetUuid,

    GenerateP256Key,
    GenerateEd255Key,
    GenerateX255Key,

    SaveP256AttestationCertificate,
    SaveEd255AttestationCertificate,
    SaveX255AttestationCertificate,

    SaveT1IntermediatePublicKey,
}

impl TryFrom<u8> for Instruction {
    type Error = Error;

    fn try_from(ins: u8) -> Result<Self, Self::Error> {
        Ok(match ins {
            0xa4 => Self::Select,
            0xd0 => Self::WriteBinary,

            0xbf => Self::WriteFile,

            0x51 => Self::BootToBootrom,
            0xbd => Self::ReformatFilesystem,
            0x62 => Self::GetUuid,

            0xbc => Self::GenerateP256Key,
            0xbb => Self::GenerateEd255Key,
            0xb7 => Self::GenerateX255Key,

            0xba => Self::SaveP256AttestationCertificate,
            0xb9 => Self::SaveEd255AttestationCertificate,
            0xb6 => Self::SaveX255AttestationCertificate,

            0xb5 => Self::SaveT1IntermediatePublicKey,

            _ => return Err(Error::FunctionNotSupported),
        })
    }
}

pub enum Error {
    FunctionNotSupported,
    IncorrectDataParameter,
    NotEnoughMemory,
    NotFound,
}

type Uuid = [u8; 16];

const FILENAME_T1_PUBLIC: &Path = path!("/attn/pub/00");

const FILENAME_P256_SECRET: &Path = path!("/attn/sec/01");
const FILENAME_ED255_SECRET: &Path = path!("/attn/sec/02");
const FILENAME_X255_SECRET: &Path = path!("/attn/sec/03");

const FILENAME_P256_CERT: &Path = path!("/attn/x5c/01");
const FILENAME_ED255_CERT: &Path = path!("/attn/x5c/02");
const FILENAME_X255_CERT: &Path = path!("/attn/x5c/03");

enum SelectedBuffer {
    Filename,
    File,
}

pub struct Provisioner<S, FS, T>
where
    S: Store,
    FS: 'static + LfsStorage,
    T: client::CryptoClient,
{
    trussed: T,

    selected_buffer: SelectedBuffer,
    buffer_filename: Vec<u8, 128>,
    buffer_file_contents: Vec<u8, 8192>,

    store: S,
    stolen_filesystem: &'static mut FS,
    #[allow(dead_code)]
    is_passive: bool,
    uuid: Uuid,
    rebooter: fn() -> !,
}

impl<S, FS, T> Provisioner<S, FS, T>
where
    S: Store,
    FS: 'static + LfsStorage,
    T: client::CryptoClient,
{
    pub fn new(
        trussed: T,
        store: S,
        stolen_filesystem: &'static mut FS,
        is_passive: bool,
        uuid: Uuid,
        rebooter: fn() -> !,
    ) -> Provisioner<S, FS, T> {
        Self {
            trussed,
            selected_buffer: SelectedBuffer::Filename,
            buffer_filename: Vec::new(),
            buffer_file_contents: Vec::new(),
            store,
            stolen_filesystem,
            is_passive,
            uuid,
            rebooter,
        }
    }

    fn handle<const N: usize>(
        &mut self,
        instruction: Instruction,
        data: &[u8],
        reply: &mut Vec<u8, N>,
    ) -> Result<(), Error> {
        match instruction {
            Instruction::Select => self.select(data),
            Instruction::WriteBinary => {
                match self.selected_buffer {
                    SelectedBuffer::Filename => self.buffer_filename.extend_from_slice(data),
                    SelectedBuffer::File => self.buffer_file_contents.extend_from_slice(data),
                }
                .unwrap();
                Ok(())
            }
            Instruction::ReformatFilesystem => {
                // Provide a method to reset the FS.
                info!("Reformatting the FS..");
                littlefs2::fs::Filesystem::format(self.stolen_filesystem)
                    .map_err(|_| Error::NotEnoughMemory)?;
                Ok(())
            }
            Instruction::WriteFile => {
                if self.buffer_file_contents.is_empty() || self.buffer_filename.is_empty() {
                    Err(Error::IncorrectDataParameter)
                } else if let Ok(buffer_path) = PathBuf::try_from(self.buffer_filename.as_slice()) {
                    // self.buffer_filename.push(0);
                    let _filename =
                        unsafe { core::str::from_utf8_unchecked(self.buffer_filename.as_slice()) };
                    info!(
                        "writing file {} {} bytes",
                        _filename,
                        self.buffer_file_contents.len()
                    );
                    // logging::dump_hex(&self.buffer_file_contents, self.buffer_file_contents.len());

                    let res = store::store(
                        &self.store,
                        trussed::types::Location::Internal,
                        &buffer_path,
                        &self.buffer_file_contents,
                    );
                    self.buffer_file_contents.clear();
                    self.buffer_filename.clear();
                    if res.is_err() {
                        info!("failed writing file!");
                        Err(Error::NotEnoughMemory)
                    } else {
                        info!("wrote file");
                        Ok(())
                    }
                } else {
                    Err(Error::IncorrectDataParameter)
                }
            }
            Instruction::GenerateP256Key => {
                use p256_cortex_m4::{Keypair, SecretKey};
                info!("GenerateP256Key");
                let mut seed = [0u8; 32];

                // Generate a keypair with rejection sampling.
                // This should use the proper `random` method but is not possible without a `CryptoRng` implementation, which trussed is not
                let keypair = loop {
                    seed.copy_from_slice(syscall!(self.trussed.random_bytes(32)).bytes.as_slice());
                    match SecretKey::from_bytes(&seed) {
                        Ok(secret) => {
                            break Keypair {
                                public: secret.public_key(),
                                secret,
                            }
                        }
                        Err(_) => continue,
                    }
                };

                let serialized_key = Key {
                    flags: Flags::LOCAL | Flags::SENSITIVE,
                    kind: KeyKind::P256,
                    material: Vec::from_slice(&seed).unwrap(),
                };

                let serialized_bytes = serialized_key.serialize();

                store::store(
                    &self.store,
                    trussed::types::Location::Internal,
                    FILENAME_P256_SECRET,
                    &serialized_bytes,
                )
                .map_err(|_| Error::NotEnoughMemory)?;
                info!("stored to {}", FILENAME_P256_SECRET.as_str());

                reply
                    .extend_from_slice(&keypair.public.to_untagged_bytes())
                    .unwrap();
                Ok(())
            }
            Instruction::GenerateEd255Key => {
                info!("GenerateEd255Key");
                let mut seed = [0u8; 32];
                seed.copy_from_slice(syscall!(self.trussed.random_bytes(32)).bytes.as_slice());

                let serialized_key = Key {
                    flags: Flags::LOCAL | Flags::SENSITIVE,
                    kind: KeyKind::Ed255,
                    material: Vec::from_slice(&seed).unwrap(),
                };

                // let serialized_key = Key::try_deserialize(&seed[..])
                // .map_err(|_| Error::WrongLength)?;

                let serialized_bytes = serialized_key.serialize();

                store::store(
                    &self.store,
                    trussed::types::Location::Internal,
                    FILENAME_ED255_SECRET,
                    &serialized_bytes,
                )
                .map_err(|_| Error::NotEnoughMemory)?;

                let keypair = salty::Keypair::from(&seed);

                reply.extend_from_slice(keypair.public.as_bytes()).unwrap();
                Ok(())
            }
            Instruction::GenerateX255Key => {
                info_now!("GenerateX255Key");
                let mut seed = [0u8; 32];
                seed.copy_from_slice(syscall!(self.trussed.random_bytes(32)).bytes.as_slice());

                let serialized_key = Key {
                    flags: Flags::LOCAL | Flags::SENSITIVE,
                    kind: KeyKind::X255,
                    material: Vec::from_slice(&seed).unwrap(),
                };

                // let serialized_key = Key::try_deserialize(&seed[..])
                // .map_err(|_| Error::WrongLength)?;

                let serialized_bytes = serialized_key.serialize();

                store::store(
                    &self.store,
                    trussed::types::Location::Internal,
                    FILENAME_X255_SECRET,
                    &serialized_bytes,
                )
                .map_err(|_| Error::NotEnoughMemory)?;

                let secret_key = salty::agreement::SecretKey::from_seed(&seed);
                let public_key = salty::agreement::PublicKey::from(&secret_key);

                reply.extend_from_slice(&public_key.to_bytes()).unwrap();
                Ok(())
            }
            Instruction::SaveP256AttestationCertificate => {
                if !self.store.ifs().exists(FILENAME_P256_SECRET) || data.len() < 100 {
                    // Assuming certs will always be >100 bytes
                    Err(Error::IncorrectDataParameter)
                } else {
                    info!("saving P256 CERT, {} bytes", data.len());
                    store::store(
                        &self.store,
                        trussed::types::Location::Internal,
                        FILENAME_P256_CERT,
                        data,
                    )
                    .map_err(|_| Error::NotEnoughMemory)?;
                    Ok(())
                }
            }
            Instruction::SaveEd255AttestationCertificate => {
                if !self.store.ifs().exists(FILENAME_ED255_SECRET) || data.len() < 100 {
                    // Assuming certs will always be >100 bytes
                    Err(Error::IncorrectDataParameter)
                } else {
                    info!("saving ED25519 CERT, {} bytes", data.len());
                    store::store(
                        &self.store,
                        trussed::types::Location::Internal,
                        FILENAME_ED255_CERT,
                        data,
                    )
                    .map_err(|_| Error::NotEnoughMemory)?;
                    Ok(())
                }
            }
            Instruction::SaveX255AttestationCertificate => {
                if !self.store.ifs().exists(FILENAME_X255_SECRET) || data.len() < 100 {
                    // Assuming certs will always be >100 bytes
                    Err(Error::IncorrectDataParameter)
                } else {
                    info!("saving X25519 CERT, {} bytes", data.len());
                    store::store(
                        &self.store,
                        trussed::types::Location::Internal,
                        FILENAME_X255_CERT,
                        data,
                    )
                    .map_err(|_| Error::NotEnoughMemory)?;
                    Ok(())
                }
            }
            Instruction::SaveT1IntermediatePublicKey => {
                info!("saving T1 INTERMEDIATE PUBLIC KEY, {} bytes", data.len());
                if data.len() != 64 {
                    Err(Error::IncorrectDataParameter)
                } else {
                    let serialized_key = Key {
                        flags: Default::default(),
                        kind: KeyKind::P256,
                        material: Vec::from_slice(data).unwrap(),
                    };

                    let serialized_key = serialized_key.serialize();

                    store::store(
                        &self.store,
                        trussed::types::Location::Internal,
                        FILENAME_T1_PUBLIC,
                        &serialized_key,
                    )
                    .map_err(|_| Error::NotEnoughMemory)
                }
            }
            Instruction::GetUuid => {
                // Get UUID
                reply
                    .extend_from_slice(&self.uuid)
                    .expect("failed copying UUID");
                Ok(())
            }
            Instruction::BootToBootrom => {
                (self.rebooter)();
            }
        }
    }

    fn select(&mut self, data: &[u8]) -> Result<(), Error> {
        if data.starts_with(&TESTER_FILENAME_ID) {
            info!("select filename");
            self.selected_buffer = SelectedBuffer::Filename;
            Ok(())
        } else if data.starts_with(&TESTER_FILE_ID) {
            info!("select file");
            self.selected_buffer = SelectedBuffer::File;
            Ok(())
        } else {
            info!("unknown ID: {:?}", data);
            Err(Error::NotFound)
        }
    }
}
