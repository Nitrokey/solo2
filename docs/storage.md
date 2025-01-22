# Storage Overview

The Nitrokey 3 has three storage types: internal, external and volatile.  The internal storage uses the on-chip flash memory.  The external storage uses a separate flash chip.  The volatile storage uses RAM.

## Storage Sizes

| Storage          | Size   |
| ---------------- | -----: |
| Internal (lpc55) | 42 KiB |
| Internal (nrf52) | 80 KiB |
| External         | 2 MiB  |

### External Flash

The external flash is not entirely under littlefs2's control. At the end
128KiB are left free for any potential future use-cases. This leaves 
1920KiB for littlefs2 usage. 

## Usage

This section describes how the storage is used in the current stable firmware.

### Trussed

Trussed stores the RNG state on the internal filesystem (see `ServiceResources::rng`).  During provisioning, a Trussed device key and certificate are also generated on the internal filesystem.

### trussed-auth

The trussed-auth extension uses the internal filesystem to store a device salt and application PINs with their metadata.

### fido-authenticator

fido-authenticator stores its state, a KEK and the resident keys on the internal filesystem.  During provisioning, the FIDO2 attestation key and certificate are stored on the internal filesystem.  The KEK is generated on first use.  If there is not enough free space to generate the KEK, the application cannot be used.

### secrets-app

secrets-app stores the user data on the external filesystem.  It uses trussed-auth for one PIN with a derived key.
