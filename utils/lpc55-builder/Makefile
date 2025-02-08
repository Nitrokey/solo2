# TODO: remove hardcoded filesystem boundary in config/flash-erase-firmware.toml

.NOTPARALLEL:

RUNNER ?= ../../runners/embedded/
OUTPUT_BIN ?= $(RUNNER)/artifacts/runner-lpc55-nk3xn.bin
OUTPUT_ELF ?= $(RUNNER)/artifacts/runner-lpc55-nk3xn.elf

.PHONY: build
build:
	$(MAKE) -C "$(RUNNER)" build-nk3xn

build-all:
	mkdir -p artifacts
	$(MAKE) build FEATURES=develop,provisioner,$(FEATURES)
	cp "$(OUTPUT_BIN)" ./artifacts/provisioner.bin
	$(MAKE) build FEATURES=$(FEATURES)
	cp "$(OUTPUT_BIN)" ./artifacts/firmware.bin
	ls -lh artifacts/
	readlink -f artifacts/
	date
	git describe --long

.PHONY: jlink
jlink:
	JLinkGDBServerCLExe -strict -device LPC55S69 -if SWD -vd

.PHONY: run
run: build
	arm-none-eabi-gdb -q -x ./jlink.gdb "$(OUTPUT_ELF)"

.PHONY: provision-release
provision-release:
	# TODO: add secure boot
	# Step 0: reset CMPA, erase firmware
	./scripts/boot-to-bootrom.sh
	./scripts/usbwait.sh 1fc9:0021 20a0:42dd
	$(MAKE) bl-config-cmpa-empty
	$(MAKE) bl-erase-firmware
	lpc55 reboot
	./scripts/usbwait.sh 1fc9:0021
	# Step 1: build & flash provisioner, provision keystore, cmpa
	$(MAKE) bl-provision-keystore
	lpc55 reboot
	./scripts/usbwait.sh 1fc9:0021
	$(MAKE) bl-config-cmpa-develop
	lpc55 reboot
	./scripts/usbwait.sh 20a0:42dd
	$(MAKE) build FEATURES=provisioner,$(FEATURES)
	$(MAKE) bl-flash
	lpc55 reboot
	./scripts/usbwait.sh 20a0:42b2
	# Step 2: provision certs
	$(MAKE) fw-provision-certs
	./scripts/boot-to-bootrom.sh
	./scripts/usbwait.sh 20a0:42dd
	# Step 3: erase firmware, build & flash final firmware
	$(MAKE) bl-erase-firmware
	$(MAKE) build FEATURES=$(FEATURES)
	$(MAKE) bl-flash
	lpc55 reboot
	./scripts/usbwait.sh 20a0:42b2

.PHONY: flash
flash: build
	./scripts/boot-to-bootrom.sh
	./scripts/usbwait.sh 1fc9:0021 20a0:42dd
	$(MAKE) bl-erase-firmware
	$(MAKE) bl-flash
	lpc55 reboot

.PHONY: reset
reset:
	./scripts/boot-to-bootrom.sh
	./scripts/usbwait.sh 1fc9:0021 20a0:42dd
	$(MAKE) bl-erase-all
	$(MAKE) bl-config-cmpa-empty
	lpc55 reboot

# The following targets prefixed with bl- require that the device is in bootloader mode.

.PHONY: bl-flash
bl-flash:
	lpc55 write-flash "$(OUTPUT_BIN)"

.PHONY: bl-erase-all
bl-erase-all:
	lpc55 provision ./config/flash-erase-all.toml

.PHONY: bl-erase-firmware
bl-erase-firmware:
	lpc55 provision ./config/flash-erase-firmware.toml

.PHONY: bl-config-cmpa-empty
bl-config-cmpa-empty:
	lpc55 configure factory-settings ./config/cmpa-empty.toml

.PHONY: bl-config-cmpa-develop
bl-config-cmpa-develop:
	lpc55 configure factory-settings ./config/cmpa-develop.toml

.PHONY: bl-config-cmpa-release
bl-config-cmpa-release:
	lpc55 configure factory-settings ./config/cmpa-release.toml

.PHONY: bl-config-cfpa
bl-config-cfpa:
	lpc55 configure customer-settings ./config/cfpa.toml

.PHONY: bl-config-cmpa
bl-config-cmpa:
	lpc55 configure factory-settings ./config/cmpa.toml

.PHONY: bl-provision-keystore
bl-provision-keystore:
	lpc55 provision ./config/keystore.toml

# The following targets prefixed with fw- require that the device is in firmware mode.

.PHONY: fw-provision-certs
fw-provision-certs: data/fido.key data/fido.cert
	# TODO: add Trussed key & cert
	solo2 app provision store-fido-batch-cert data/fido.cert
	solo2 app provision store-fido-batch-key data/fido.key

data/fido.key: data/fido.cert

data/fido.cert:
	mkdir -p data
	solo2 pki dev fido data/fido.key data/fido.cert
