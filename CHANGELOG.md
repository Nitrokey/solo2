# v1.0.2 (2022-01-26)
No changes since rc.1.

# v1.0.2-rc.1 (2022-01-25)

Update to upstream release 1.0.9.

## Bugfixes

- usbd-ctaphid: fix ctaphid keepalive messages - fixes "busy" issue under Windows  ([#21][]) 


[#21]: https://github.com/Nitrokey/nitrokey-3-firmware/issues/21

# v1.0.1 (2022-01-15)

## Bugfixes
- fido-authenticator: use smaller CredentialID - fixes issues with some services FIDO usage ([fido-authenticator#8][])
- trussed: update P256 library - fixes signing failure in some cases ([#31][])

[#31]: https://github.com/Nitrokey/nitrokey-3-firmware/issues/31
[fido-authenticator#8]: https://github.com/solokeys/fido-authenticator/pull/8

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