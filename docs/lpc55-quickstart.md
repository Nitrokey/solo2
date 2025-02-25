# LPC55 Quickstart Guide

This guide explains how to compile and flash the firmware on a LPC55 Nitrokey 3 Hacker device using a Linux system.

*Note:*  This guide and the building tools are work in progress and may change frequently.

## Requirements

* Hardware
  * Nitrokey 3A NFC Hacker (NK3ANH) or a similar device with a LPC55 MCU
  * optional: [LPC-Link2](https://www.embeddedartists.com/products/lpc-link2/)
* Software
  * Python 3 with the `toml` package
  * [rustup](https://rustup.rs)
  * GNU Make
  * Git
  * LLVM, Clang, GCC
    * On Debian-based distributions, you need to install these packages: `llvm clang libclang-dev gcc-arm-none-eabi libc6-dev-i386`
  * PC/SC Smart Card Daemon [`pcsclite`](https://pcsclite.apdu.fr/) (needed for `solo2`)
    * On Debian-based distributions, you neeed to install this package: `pcscd`
  * [`nitropy`](https://github.com/nitrokey/pynitrokey)
  * [`solo2`](https://github.com/solokeys/solo2-cli) with the `dev-pki` feature
  * [`lpc55`](https://github.com/lpc55/lpc55-host)
  * [`flip-link`](https://github.com/knurling-rs/flip-link)

You can order a NK3ANH at [shop@nitrokey.com](mailto:shop@nitrokey.com).

## Introduction

The LPC55 MCU has two operation modes:  a bootloader mode that can be used to configure the device and to write, read and erase the internal flash; and a firmware mode that executes the firmware stored in the internal flash.  If no firmware has been flashed, it boots into bootloader mode.  The Nitrokey 3 firmware also provides a command to boot into the bootloader mode.  In case the firmware is broken, you can boot into bootloader mode by physically activating a special pin.

For the Nitrokey 3, we use two configuration sets:  A simple development configuration, and a release configuration using hardware encryption and secure boot.  This guide currently only describes the development configuration.

To get started with the NK3ANH, you have to perform the following steps:

1. Reset the device
2. Apply the development configuration
3. Flash a provisioning firmware
4. Generate and provision FIDO attestation key and certificate
5. Flash the final firmware

For a regular Nitrokey 3 device, we would also perform these steps that are currently not covered by this guide:

- Apply the release configuration and generate device secrets
- Generate, sign and provision a Trussed device key and certificate
- Seal the device configuration (not reversible)

## Getting Started

### Compiling the Firmware

Clone the firmware repository [nitrokey/nitrokey-3-firmware](https://github.com/nitrokey/nitrokey-3-firmware):

```
$ git clone https://github.com/nitrokey/nitrokey-3-firmware
$ cd nitrokey-3-firmware
```

Install the required Rust toolchain:

```
$ rustup target add thumbv8m.main-none-eabi
```

Make sure that you can compile the firmware:

```
$ make -C runners/embedded build-nk3xn
```

### Preparing the Device

Disconnect all Nitrokey 3 devices.  It is recommended to also disconnect other FIDO and smartcard devices.  Connect your NK3ANH device while monitoring your system log, for example with `dmesg --follow`.  You should see one of the following USB devices appear:
- NXP SEMICONDUCTOR INC. USB COMPOSITE DEVICE (idVendor=1fc9, idProduct=0021)
- NXP SEMICONDUCTOR INC. USB COMPOSITE DEVICE (idVendor=20a0, idProduct=42dd)
- Nitrokey Nitrokey 3 (idVendor=20a0, idProduct=42b2)

Now make sure that the device is listed in the output of `lpc55 ls` (first and second case) and/or `nitropy nk3 list` (second and third case).  If it is not listed, you have to install
the [NXP](https://spsdk.readthedocs.io/en/latest/examples/_knowledge_base/installation_guide.html#usb-under-linux) (first case) and/or [Nitrokey](https://docs.nitrokey.com/software/nitropy/linux/udev) udev rules (second and third case). For the first case, you can alternatively use the file `utils/lpc55-builder/70-lpc55.rules` from the firmware repository.

To make sure that that the device is in a clean state, reset it:
```
$ make -C utils/lpc55-builder reset
```

### Flashing and Provisioning the Device

Build the firmware, configure the device and flash the firmware by running:
```
$ make -C utils/lpc55-builder provision-release
```

Now check that everything worked by running `nitropy nk3 test --exclude provisioner`.  The `uuid` and `version` checks should be successful.  The `fido2` test will fail with a message about an unexpected certificate hash.  This is expected because you don’t have access to the real FIDO2 batch keys and certificates.  If it fails with a different error message, something went wrong.

```
[1/3]   uuid            UUID query                      SUCCESS         223FE5E2AE287150AD9DAD9E34B7F989
[2/3]   version         Firmware version query          SUCCESS         v1.2.2
Please press the touch button on the device ...
[3/3]   fido2           FIDO2                           FAILURE         Unexpected FIDO2 cert hash for version v1.2.2: c7fbd9ee89f3a32408ce6cc4adb23da940cc1515c741237ef4b8718e24515ac6
```

### Updating the Firmware

After the initial provisioning, you can re-build and update the firmware with:
```
$ make -C utils/lpc55-builder flash FEATURES=develop
```
You might have to press the touch button during the command to confirm a reboot.

## Using the Test Firmware

The test firmware can be built by activating the `test` feature.  First, make sure that you can compile the test firmware:
```
$ make -C runners/embedded build-nk3xn FEATURES=test
```

If you have followed the provisioning steps in the Getting Started section, you should now be able to update to the test firmware:
```
$ make -C utils/lpc55-builder flash FEATURES=develop,test
```

## Debugging

The NK3ANH can be debugged using SWD. The SWD interface is exposed over the GND, (SW)DIO and (SW)CLK pins. An external debugger is required, for example [LPC-Link2](https://www.embeddedartists.com/products/lpc-link2/).

### Preparing the Connection

* Remove one of the connectors from a 2x5 SWD cable.
* Solder the SWD cables to the pads on the Nitrokey board:
  * Cable 2 (SWDIO): DIO
  * Cable 3 (GND): GND
  * Cable 4 (SWDCLK): CLK
* Connect the cable to the J7 socket on the debugger.

Alternatively, use a [breakout connector](https://www.adafruit.com/product/2743).

### J-LINK

#### Install the J-LINK firmware

(See also the [NXP Debug Probe Firmware Programming Guide](https://www.nxp.com/docs/en/supporting-information/Debug_Probe_Firmware_Programming.pdf)).

* Close JP1 and connect the board.
* Check the output of `lsusb -d 1366:0101`. You should see a SEGGER J-Link PLUS device. If the device is present, you can skip this section. If it is not present, you have to update the debugger firmware as described in this section.
* Download and install `lpcscrypt`.
* Check the [Segger LPC-Link2 site](https://www.segger.com/lpc-link-2.html) for updated firmware images.
* Disconnect the board, open JP1 and reconnect the board to the computer.
* Run `/usr/local/lpcscrypt/scripts/program_JLINK`.
* Disconnect the board, close JP1 and reconnect the board to the computer. Now the device should appear in the `lsusb` output.

#### Running the Debugger

* Close JP2.
* Install the [JLink Software and Documentation pack](https://www.segger.com/downloads/jlink/#J-LinkSoftwareAndDocumentationPack).
* Execute `make -C utils/lpc55-builder jlink`.
* Execute `make -C utils/lpc55-builder run` to flash the firmware and execute it with gdb.

## Troubleshooting

### Force Bootloader Mode

If your device is unresponsive, you can force it to boot into bootloader mode by activating the bootloader pin while you connect the device as described in [this comment](https://github.com/Nitrokey/nitrokey-3-firmware/issues/112#issuecomment-1323828805).

TODO: include guide and photos
