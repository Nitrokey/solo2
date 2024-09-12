use crate::{Instruction, Provisioner};
use core::convert::TryFrom;
use ctaphid_dispatch::{
    app::App,
    command::{Command, VendorCommand},
    types::{Error, Message},
};
use trussed::{client, store::Store, Client};

const COMMAND_PROVISIONER: VendorCommand = VendorCommand::H71;

impl<S, T> App<'static> for Provisioner<S, T>
where
    S: Store,
    T: Client + client::X255 + client::HmacSha256,
{
    fn commands(&self) -> &'static [Command] {
        &[Command::Vendor(COMMAND_PROVISIONER)]
    }

    fn call(
        &mut self,
        command: Command,
        request: &Message,
        response: &mut Message,
    ) -> Result<(), Error> {
        if command != Command::Vendor(COMMAND_PROVISIONER) {
            return Err(Error::InvalidCommand);
        }
        if request.is_empty() {
            return Err(Error::InvalidLength);
        }
        Instruction::try_from(request[0])
            .and_then(|instruction| self.handle(instruction, &request[1..], response))
            .map_err(|_| Error::InvalidCommand)
    }
}
