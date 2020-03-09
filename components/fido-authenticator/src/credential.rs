use cortex_m_semihosting::hprintln;

use core::convert::TryFrom;

use crypto_service::{
    types::{
        ObjectHandle,
    },
};

use ctap_types::{
    Bytes, consts, String, Vec,
    // authenticator::{ctap1, ctap2, Error, Request, Response},
    authenticator::ctap2,
};

use super::{Error, Result};

#[derive(Copy, Clone, Debug, serde::Deserialize, serde::Serialize)]
// #[derive(Copy, Clone, Debug, serde_indexed::DeserializeIndexed, serde_indexed::SerializeIndexed)]
pub enum CtapVersion {
    U2fV2,
    Fido20,
    Fido21Pre,
}

#[derive(Clone, Debug, Default)]
pub struct CredentialId(pub crypto_service::types::MediumData);

// TODO: how to determine necessary size?
// pub type SerializedCredential = Bytes<consts::U512>;
// pub type SerializedCredential = Bytes<consts::U256>;
pub type SerializedCredential = crypto_service::types::Message;

#[derive(Clone, Debug)]
pub struct EncryptedSerializedCredential(pub crypto_service::api::reply::Encrypt);

impl TryFrom<EncryptedSerializedCredential> for CredentialId {
    type Error = Error;

    // fn try_from(esc: EncryptedSerializedCredential) -> Result<CredentialId> {
    //     let mut credential_id = crypto_service::types::Message::new();
    //     credential_id.extend_from_slice(&esc.0.tag).map_err(|_| Error::Other)?;
    //     credential_id.extend_from_slice(&esc.0.nonce).map_err(|_| Error::Other)?;
    //     credential_id.extend_from_slice(&esc.0.ciphertext).map_err(|_| Error::Other)?;
    //     Ok(CredentialId(credential_id))
    // }

    fn try_from(esc: EncryptedSerializedCredential) -> Result<CredentialId> {
        let mut credential_id = CredentialId::default();
        credential_id.0.resize_to_capacity();
        let buffer = &mut credential_id.0;
        // let size = ctap_types::serde::cbor_serialize(&esc.0, buffer).map_err(|_| Error::Other)?;
        let size = ctap_types::serde::cbor_serialize(&esc.0, buffer).unwrap();
        credential_id.0.resize_default(size);
        Ok(credential_id)
    }
}

impl TryFrom<CredentialId> for EncryptedSerializedCredential {
    // tag = 16B
    // nonce = 12B
    type Error = Error;

    fn try_from(cid: CredentialId) -> Result<EncryptedSerializedCredential> {
        let encrypted_serialized_credential = EncryptedSerializedCredential(
            ctap_types::serde::cbor_deserialize(&cid.0).map_err(|_| Error::Other)?
        );
        Ok(encrypted_serialized_credential)
    }

    // fn try_from(cid: CredentialId) -> Result<EncryptedSerializedCredential> {
    //     if cid.0.len() < 28 {
    //         return Err(Error::InvalidCredential);
    //     }
    //     let tag = &cid.0[..16];
    //     let nonce = &cid.0[16..][..12];
    //     let cipher = &cid.0[28..];
    //     Ok(EncryptedSerializedCredential(crypto_service::api::reply::Encrypt {
    //         ciphertext: {
    //             let mut c = crypto_service::types::Message::new();
    //             c.extend_from_slice(cipher).map_err(|_| Error::Other)?;
    //             c
    //         },
    //         nonce: {
    //             let mut c = crypto_service::types::ShortData::new();
    //             c.extend_from_slice(nonce).map_err(|_| Error::Other)?;
    //             c
    //         },
    //         tag: {
    //             let mut c = crypto_service::types::ShortData::new();
    //             c.extend_from_slice(tag).map_err(|_| Error::Other)?;
    //             c
    //         },
    //     }))
    // }
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub enum Key {
    ResidentKey(ObjectHandle),
    WrappedKey(Bytes<consts::U32>),
}

// TODO: figure out sizes
// We may or may not follow https://github.com/satoshilabs/slips/blob/master/slip-0022.md
#[derive(Clone, Debug, serde_indexed::DeserializeIndexed, serde_indexed::SerializeIndexed)]
#[serde_indexed(offset = 1)]
pub struct Credential {
    ctap: i32, //CtapVersion,

    // id, name, url
    rp: ctap_types::webauthn::PublicKeyCredentialRpEntity,
    // id, name, display_name
    user: ctap_types::webauthn::PublicKeyCredentialUserEntity,

    // can be just a counter, need to be able to determine "latest"
    creation_time: u32,
    // for deterministic keys, it seems CTAP2 (but not CTAP1) makes signature counters optional
    use_counter: bool,
    // P256 or Ed25519
    algorithm: i32,
    // for RK in non-deterministic mode: refers to actual key
    // TODO(implement enums in cbor-deser): for all others, is a wrapped key
    // --> use above Key enum
    // #[serde(skip_serializing_if = "Option::is_none")]
    // key_id: Option<ObjectHandle>,
    the_key: Key,

    // extensions
    hmac_secret: bool,
    cred_protect: bool,
}

impl Credential {
    pub fn new(
        ctap: CtapVersion,
        parameters: &ctap2::make_credential::Parameters,
        algorithm: i32,
        key: Key,
        timestamp: u32,
        hmac_secret: bool,
        cred_protect: bool,
    )
        -> Self
    {
        Credential {
            // ctap,
            ctap: ctap as i32,

            rp: parameters.rp.clone(),
            user: parameters.user.clone(),

            creation_time: timestamp,
            use_counter: true,
            algorithm: algorithm,
            the_key: key,

            hmac_secret,
            cred_protect,
        }
    }

    pub fn serialize(&self) -> Result<SerializedCredential> {
        let mut serialized = SerializedCredential::new();
        serialized.resize_to_capacity();
        let buffer = &mut serialized;
        let size = ctap_types::serde::cbor_serialize(self, buffer).map_err(|_| Error::Other)?;
        serialized.resize_default(size);
        Ok(serialized)
    }

    pub fn deserialize(bytes: &SerializedCredential) -> Result<Self> {
        // ctap_types::serde::cbor_deserialize(bytes).map_err(|_| Error::Other)
        Ok(ctap_types::serde::cbor_deserialize(bytes).unwrap())
    }
}
