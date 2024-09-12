use crate::{Error, Provisioner};
use apdu_dispatch::{
    app::{App, Interface, Result},
    command::SIZE as CommandSize,
    iso7816::{self, Aid, Instruction, Status},
    response,
    response::SIZE as ResponseSize,
    Command,
};
use core::convert::{TryFrom, TryInto};
use trussed::{client, store::Store, Client};

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

impl<S, T> iso7816::App for Provisioner<S, T>
where
    S: Store,
    T: Client + client::X255 + client::HmacSha256,
{
    fn aid(&self) -> Aid {
        Aid::new(SOLO_PROVISIONER_AID)
    }
}

impl<S, T> App<CommandSize, ResponseSize> for Provisioner<S, T>
where
    S: Store,
    T: Client + client::X255 + client::HmacSha256,
{
    fn select(
        &mut self,
        _interface: Interface,
        _apdu: &Command,
        reply: &mut response::Data,
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
        apdu: &Command,
        reply: &mut response::Data,
    ) -> Result {
        apdu.instruction()
            .try_into()
            .and_then(|instruction| self.handle(instruction, apdu.data(), reply))
            .map_err(From::from)
    }
}
