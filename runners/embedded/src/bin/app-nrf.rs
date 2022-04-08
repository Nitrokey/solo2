#![no_std]
#![no_main]

use embedded_runner_lib as ERL;

#[macro_use]
extern crate delog;
delog::generate_macros!();

delog!(Delogger, 3 * 1024, 512, ERL::types::DelogFlusher);

#[rtic::app(device = nrf52840_hal::pac, peripherals = true, dispatchers = [SWI3_EGU3, SWI4_EGU4, SWI5_EGU5])]
mod app {
    use super::{Delogger, ERL, ERL::soc::rtic_monotonic::RtcDuration};
    use nrf52840_hal::{
        gpio::{p0, p1},
        gpiote::Gpiote,
        rng::Rng,
        timer::Timer,
    };
    use rand_core::SeedableRng;
    use trussed::{syscall, Interchange};

    #[shared]
    struct SharedResources {
        apps: ERL::types::Apps,
        apdu_dispatch: ERL::types::ApduDispatch,
        ctaphid_dispatch: ERL::types::CtaphidDispatch,
        usb_classes: Option<ERL::types::usbnfc::UsbClasses>,
        contactless: Option<ERL::types::Iso14443>,
        trussed: ERL::types::Trussed,
        /* NRF specific elements */
        // (display UI)
        // (fingerprint sensor)
        // (SE050)
        /* NRF specific device peripherals */

        /* LPC55 specific elements */
        // perf_timer
        // clock_ctrl
        // wait_extender
    }

    #[local]
    struct LocalResources {
        trussed_client: ERL::types::TrussedClient,
        gpiote: Gpiote,
        power: nrf52840_pac::POWER,
        ui_counter: u16,
    }

    #[monotonic(binds = RTC0, default = true)]
    type RtcMonotonic = ERL::soc::rtic_monotonic::RtcMonotonic;

    #[init()]
    fn init(mut ctx: init::Context) -> (SharedResources, LocalResources, init::Monotonics) {
        ctx.core.DCB.enable_trace();
        ctx.core.DWT.enable_cycle_counter();

        rtt_target::rtt_init_print!();
        Delogger::init_default(delog::LevelFilter::Trace, &ERL::types::DELOG_FLUSHER).ok();
        ERL::banner();

        ERL::soc::init_bootup(&ctx.device.FICR, &ctx.device.UICR, &mut ctx.device.POWER);

        let mut delay_timer = Timer::<nrf52840_pac::TIMER0>::new(ctx.device.TIMER0);

        let dev_gpiote = Gpiote::new(ctx.device.GPIOTE);
        let mut board_gpio = {
            let dev_gpio_p0 = p0::Parts::new(ctx.device.P0);
            let dev_gpio_p1 = p1::Parts::new(ctx.device.P1);
            ERL::soc::board::init_pins(&dev_gpiote, dev_gpio_p0, dev_gpio_p1)
        };
        dev_gpiote.reset_events();
        dev_gpiote.port().enable_interrupt();

        /* check reason for booting */
        let reset_reason = ctx.device.POWER.resetreas.read().bits();
        ctx.device.POWER.resetreas.write(|w| w);

        let wakeup_by_nfc: bool = reset_reason == 0x0008_0000;
        /* a) powered through NFC: enable NFC, keep external oscillator off, don't start USB */
        /* b) powered through USB: start external oscillator, start USB, keep NFC off(?) */

        ERL::soc::setup_lfclk_hfxo(ctx.device.CLOCK);
        let usbd_ref = ERL::soc::setup_usb_bus(ctx.device.USBD);
        let nfc_ref = ERL::soc::nfc::NrfNfc::new(ctx.device.NFCT);
        let usbnfcinit = ERL::init_usb_nfc(Some(usbd_ref), Some(nfc_ref));

        let internal_flash = ERL::soc::init_internal_flash(ctx.device.NVMC);

        #[cfg(feature = "extflash_qspi")]
        let extflash = {
            let mut qspi_extflash = ERL::soc::qspiflash::QspiFlash::new(
                ctx.device.QSPI,
                board_gpio.flashnfc_spi.take().unwrap(),
                board_gpio.flash_cs.take().unwrap(),
                board_gpio.flash_power,
                &mut delay_timer,
            );
            qspi_extflash.activate();
            trace!(
                "qspi jedec: {}",
                delog::hex_str!(&qspi_extflash.read_jedec_id())
            );

            #[cfg(feature = "qspi_destructive_test")]
            {
                use littlefs2::driver::Storage;
                let mut mybuf: [u8; 1024] = [0u8; 1024];
                mybuf[2] = 0x5a;
                trace!("qspi test 0: erase 4K @ 0x0000");
                qspi_extflash.erase(0, 0x1000).expect("qspi erase0");
                trace!("qspi test 1: read 16 @ 0x0400");
                qspi_extflash
                    .read(0x400, &mut mybuf[0..16])
                    .expect("qspi read1");
                trace!("-> read: {}", delog::hex_str!(&mybuf[0..16]));
                mybuf[0..8].copy_from_slice(&[0x55, 0xaa, 0x33, 0x99, 0x5a, 0xa5, 0x39, 0x93]);
                trace!("qspi test 2: write 55aa33995aa53993 @ 0x400");
                qspi_extflash.write(0x400, &mybuf).expect("qspi write2");
                trace!("qspi test 3: read 16 @ 0x0400");
                qspi_extflash
                    .read(0x400, &mut mybuf[0..16])
                    .expect("qspi read3");
                trace!("-> read: {}", delog::hex_str!(&mybuf[0..16]));
            }
            qspi_extflash
        };
        #[cfg(not(feature = "extflash_qspi"))]
        let extflash = {
            trace!("extflash = RAM");
            ERL::soc::types::ExternalStorage::new()
        };

        let store: ERL::types::RunnerStore = ERL::init_store(internal_flash, extflash);

        /* TODO: set up fingerprint device */
        /* TODO: set up SE050 device */

        /* *********************************** */
        /* in the meantime just test i2c comms */
        /* SE050 minimal functional test - TO BE REMOVED on SE050 inclusion*/

        use embedded_hal::blocking::delay::DelayMs;
        use nrf52840_hal::prelude::OutputPin;

        if let Some(se_ena) = &mut board_gpio.se_power {
            match se_ena.set_high() {
                Err(e) => {
                    panic!("failed setting se_power high {:?}", e);
                }
                Ok(_) => {
                    debug!("setting se_power high");
                }
            }
        }

        let mut twim = nrf52840_hal::twim::Twim::new(
            ctx.device.TWIM1,
            board_gpio.se_pins.take().unwrap(),
            nrf52840_hal::twim::Frequency::K400,
        );

        delay_timer.delay_ms(100u32);

        // RESYNC command
        let write_buf = [0x5a, 0xc0, 0x00, 0xff, 0xfc];
        match twim.write(0x48, &write_buf) {
            Err(e) => {
                panic!("i2c: failed I2C write! - {:?}", e);
            }
            Ok(_) => {
                debug!("i2c: write I2C success....");
            }
        }

        delay_timer.delay_ms(100u32);

        // RESYNC response
        let mut response = [0; 2];
        match twim.read(0x48, &mut response) {
            Err(e) => {
                panic!("i2c: failed I2C read! - {:?}", e);
            }
            Ok(_) => {
                if response == [0xa5, 0xe0] {
                    debug!("i2c: se050 activation RESYNC cool");
                } else {
                    panic!("i2c: se050 activation RESYNC fail!");
                }
            }
        }
        /* end of se050 minial functional test */
        /* *********************************** */

        /* TODO: set up display */

        let dev_rng = Rng::new(ctx.device.RNG);
        let chacha_rng = chacha20::ChaCha8Rng::from_rng(dev_rng).unwrap();

        #[cfg(feature = "board-nk3am")]
        let ui = ERL::soc::board::init_ui(
            board_gpio.rgb_led,
            ctx.device.PWM0,
            ctx.device.PWM1,
            ctx.device.PWM2,
            board_gpio.touch.unwrap(),
        );
        #[cfg(feature = "board-proto1")]
        let ui = ERL::soc::board::init_ui(
            ctx.device.SPIM0,
            board_gpio.display_spi.take().unwrap(),
            board_gpio.display_dc.take().unwrap(),
            board_gpio.display_reset.take().unwrap(),
            board_gpio.display_power,
            board_gpio.display_backlight,
            board_gpio.buttons,
            board_gpio.leds,
            &mut delay_timer,
        );
        #[cfg(feature = "board-nrfdk")]
        let ui = ERL::soc::board::init_ui();

        let platform: ERL::types::RunnerPlatform =
            ERL::types::RunnerPlatform::new(chacha_rng, store, ui);

        let mut trussed_service = trussed::service::Service::new(platform);

        let apps = ERL::init_apps(&mut trussed_service, &store, wakeup_by_nfc);
        let runner_client = {
            let (rq, rp) = trussed::pipe::TrussedInterchange::claim().unwrap();
            trussed_service.add_endpoint(rp, "runtime".into()).ok();
            let sys = ERL::types::RunnerSyscall::default();
            ERL::types::TrussedClient::new(rq, sys)
        };

        let rtc_mono = RtcMonotonic::new(ctx.device.RTC0);

        ui::spawn_after(RtcDuration::from_ms(2500)).ok();

        // compose LateResources
        (
            SharedResources {
                apps,
                apdu_dispatch: usbnfcinit.apdu_dispatch,
                ctaphid_dispatch: usbnfcinit.ctaphid_dispatch,
                usb_classes: usbnfcinit.usb_classes,
                contactless: usbnfcinit.iso14443,
                trussed: trussed_service,
            },
            LocalResources {
                trussed_client: runner_client,
                gpiote: dev_gpiote,
                power: ctx.device.POWER,
                ui_counter: 0,
            },
            init::Monotonics(rtc_mono),
        )
    }

    #[idle(shared = [apps, apdu_dispatch, ctaphid_dispatch, usb_classes, contactless])]
    fn idle(ctx: idle::Context) -> ! {
        let idle::SharedResources {
            mut apps,
            mut apdu_dispatch,
            mut ctaphid_dispatch,
            mut usb_classes,
            mut contactless,
        } = ctx.shared;

        trace!("idle");
        // TODO: figure out whether entering WFI is really worth it
        // cortex_m::asm::wfi();

        loop {
            Delogger::flush();

            let (usb_activity, _nfc_activity) = apps.lock(|apps| {
                apdu_dispatch.lock(|apdu_dispatch| {
                    ctaphid_dispatch.lock(|ctaphid_dispatch| {
                        ERL::runtime::poll_dispatchers(apdu_dispatch, ctaphid_dispatch, apps)
                    })
                })
            });
            if usb_activity {
                /*trace!("app->usb");*/
                rtic::pend(nrf52840_pac::Interrupt::USBD);
            }

            usb_classes.lock(|usb_classes| {
                ERL::runtime::poll_usb(
                    usb_classes,
                    ccid_keepalive::spawn_after,
                    ctaphid_keepalive::spawn_after,
                    monotonics::now().into(),
                );
            });

            contactless.lock(|contactless| {
                ERL::runtime::poll_nfc(contactless, nfc_keepalive::spawn_after);
            });
        }
        // loop {}
    }

    #[task(priority = 2, binds = SWI0_EGU0, shared = [trussed])]
    fn task_trussed(ctx: task_trussed::Context) {
        let mut trussed = ctx.shared.trussed;

        trace!("irq SWI0_EGU0");
        trussed.lock(|trussed| {
            ERL::runtime::run_trussed(trussed);
        });
    }

    #[task(priority = 5, binds = GPIOTE, local = [gpiote])] /* ui, fpr */
    fn task_button_irq(ctx: task_button_irq::Context) {
        let gpiote = ctx.local.gpiote;

        trace!("irq GPIOTE");
        gpiote.reset_events();
    }

    #[task(priority = 3, binds = USBD, shared = [usb_classes])]
    fn task_usb(ctx: task_usb::Context) {
        // trace!("irq USB");
        let mut usb_classes = ctx.shared.usb_classes;

        usb_classes.lock(|usb_classes| {
            ERL::runtime::poll_usb(
                usb_classes,
                ccid_keepalive::spawn_after,
                ctaphid_keepalive::spawn_after,
                monotonics::now().into(),
            );
        });
    }

    #[task(priority = 3, shared = [usb_classes])]
    fn ccid_keepalive(ctx: ccid_keepalive::Context) {
        let mut usb_classes = ctx.shared.usb_classes;

        usb_classes.lock(|usb_classes| {
            ERL::runtime::ccid_keepalive(usb_classes, ccid_keepalive::spawn_after);
        });
    }

    #[task(priority = 3, shared = [usb_classes])]
    fn ctaphid_keepalive(ctx: ctaphid_keepalive::Context) {
        let mut usb_classes = ctx.shared.usb_classes;

        usb_classes.lock(|usb_classes| {
            ERL::runtime::ctaphid_keepalive(usb_classes, ctaphid_keepalive::spawn_after);
        });
    }

    #[task(priority = 4, shared = [contactless])]
    fn nfc_keepalive(ctx: nfc_keepalive::Context) {
        let mut contactless = ctx.shared.contactless;

        contactless.lock(|contactless| {
            ERL::runtime::nfc_keepalive(contactless, nfc_keepalive::spawn_after);
        });
    }

    #[task(priority = 5, binds = POWER_CLOCK, local = [power])]
    fn power_handler(ctx: power_handler::Context) {
        let power = ctx.local.power;

        trace!(
            "irq PWR {:x} {:x} {:x}",
            power.mainregstatus.read().bits(),
            power.usbregstatus.read().bits(),
            power.pofcon.read().bits()
        );

        if power.events_usbdetected.read().events_usbdetected().bits() {
            power.events_usbdetected.write(|w| unsafe { w.bits(0) });
            trace!("usb+");
        }
        if power.events_usbpwrrdy.read().events_usbpwrrdy().bits() {
            power.events_usbpwrrdy.write(|w| unsafe { w.bits(0) });
            trace!("usbY");
        }
        if power.events_usbremoved.read().events_usbremoved().bits() {
            power.events_usbremoved.write(|w| unsafe { w.bits(0) });
            trace!("usb-");
        }
    }

    #[task(priority = 1, shared = [trussed], local = [trussed_client, ui_counter])]
    fn ui(ctx: ui::Context) {
        //trace!("UI");
        use trussed::client::GuiClient;
        let ui::LocalResources {
            trussed_client: cl,
            ui_counter: cnt,
        } = ctx.local;
        let mut trussed = ctx.shared.trussed;

        //trace!("update ui");
        trussed.lock(|trussed| {
            trussed.update_ui();
        });

        trace!("UI {} {}", *cnt, *cnt % 4);
        match *cnt % 4 {
            0 => {
                syscall!(cl.draw_filled_rect(0, 0, 240, 135, 0x0000_u16));
            }
            1 => {
                syscall!(cl.draw_text(50, 50, b"NRF52840"));
            }
            2 => {
                syscall!(cl.draw_filled_rect(0, 0, 240, 1, 0xffff_u16));
                syscall!(cl.draw_filled_rect(0, 0, 1, 135, 0xffff_u16));
                syscall!(cl.draw_filled_rect(239, 0, 1, 135, 0xffff_u16));
                syscall!(cl.draw_filled_rect(0, 134, 240, 1, 0xffff_u16));
            }
            _ => {
                syscall!(cl.draw_sprite(0, 125, 1, 9));
                syscall!(cl.draw_sprite(215, 0, 1, 6));
                syscall!(cl.draw_sprite(215, 125, 1, 7));
                syscall!(cl.draw_sprite(0, 0, 1, 8));
            }
        }
        *cnt += 1;

        ui::spawn_after(RtcDuration::from_ms(125)).ok();
    }
}
