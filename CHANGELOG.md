# v1.0.1-rc.1 (2021-12-06)

This release fixes some issues with the FIDO authenticator and the admin
application.

## Features

- Change LED color and device name if provisioner app is enabled.

## Bugfixes

- admin-app: Fix CTAPHID command dispatch ([#8][]).
- admin-app: Fix CTAPHID wink command ([#9][]).
- fido-authenticator: Handle pin protocol field in hmac-secret extension data
  to fix the authenticatorGetAssertion command for newer clients ([#14][],
  [fido-authenticator#1][]).
- fido-authenticator: Signal credential protetection ([fido-authenticator#5][]).

[#8]: https://github.com/Nitrokey/nitrokey-3-firmware/issues/8
[#9]: https://github.com/Nitrokey/nitrokey-3-firmware/issues/9
[#14]: https://github.com/Nitrokey/nitrokey-3-firmware/issues/14
[fido-authenticator#1]: https://github.com/solokeys/fido-authenticator/pull/1
[fido-authenticator#5]: https://github.com/solokeys/fido-authenticator/pull/5

# v1.0.0 (2021-10-16)

First stable firmware release with FIDO authenticator.