use crate::{Error, Provisioner};
use apdu_app::{App, CommandView, Data, Interface, Result, Status};
use core::convert::{TryFrom, TryInto};
use iso7816::{Aid, Instruction};
use trussed::{client, store::Store, types::LfsStorage};

const SOLO_PROVISIONER_AID: &[u8] = &[0xA0, 0x00, 0x00, 0x08, 0x47, 0x01, 0x00, 0x00, 0x01];

impl TryFrom<Instruction> for super::Instruction {
    type Error = Error;

    fn try_from(instruction: Instruction) -> core::result::Result<Self, Self::Error> {
        match instruction {
            Instruction::Select => Ok(Self::Select),
            Instruction::WriteBinary => Ok(Self::WriteBinary),
            Instruction::Unknown(instruction) => instruction.try_into(),
            _ => Err(Error::FunctionNotSupported),
        }
    }
}

impl From<Error> for Status {
    fn from(error: Error) -> Self {
        match error {
            Error::FunctionNotSupported => Status::FunctionNotSupported,
            Error::IncorrectDataParameter => Status::IncorrectDataParameter,
            Error::NotEnoughMemory => Status::NotEnoughMemory,
            Error::NotFound => Status::NotFound,
        }
    }
}

impl<S, FS, T> iso7816::App for Provisioner<S, FS, T>
where
    S: Store,
    FS: 'static + LfsStorage,
    T: client::CryptoClient,
{
    fn aid(&self) -> Aid {
        Aid::new(SOLO_PROVISIONER_AID)
    }
}

impl<S, FS, T, const R: usize> App<R> for Provisioner<S, FS, T>
where
    S: Store,
    FS: 'static + LfsStorage,
    T: client::CryptoClient,
{
    fn select(
        &mut self,
        _interface: Interface,
        _apdu: CommandView<'_>,
        reply: &mut Data<R>,
    ) -> Result {
        self.buffer_file_contents.clear();
        self.buffer_filename.clear();
        // For manufacture speed, return uuid on select
        reply.extend_from_slice(&self.uuid).unwrap();
        Ok(())
    }

    fn deselect(&mut self) {}

    fn call(
        &mut self,
        _interface_type: Interface,
        apdu: CommandView<'_>,
        reply: &mut Data<R>,
    ) -> Result {
        apdu.instruction()
            .try_into()
            .and_then(|instruction| self.handle(instruction, apdu.data(), reply))
            .map_err(From::from)
    }
}
