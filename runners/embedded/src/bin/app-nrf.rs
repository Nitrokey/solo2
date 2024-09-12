#![no_std]
#![no_main]

delog::generate_macros!();

use cortex_m_rt::{exception, ExceptionFrame};

#[rtic::app(device = nrf52840_hal::pac, peripherals = true, dispatchers = [SWI3_EGU3, SWI4_EGU4, SWI5_EGU5])]
mod app {
    use apdu_dispatch::{dispatch::ApduDispatch, interchanges::Channel as CcidChannel};
    use boards::{
        init::UsbClasses,
        nk3am::{self, InternalFlashStorage, NK3AM},
        runtime,
        soc::nrf52::{self, rtic_monotonic::RtcDuration},
        store::{self, StoreResources},
        Apps, Trussed,
    };
    use ctaphid_dispatch::dispatch::Dispatch as CtaphidDispatch;
    use interchange::Channel;
    use nrf52840_hal::{gpiote::Gpiote, rng::Rng};

    use embedded_runner_lib::{VERSION, VERSION_STRING};

    type Board = NK3AM;

    type Soc = <Board as boards::Board>::Soc;

    #[shared]
    struct SharedResources {
        trussed: Trussed<Board>,
        apps: Apps<Board>,
        apdu_dispatch: ApduDispatch<'static>,
        ctaphid_dispatch: CtaphidDispatch<'static, 'static>,
        usb_classes: Option<UsbClasses<Soc>>,
    }

    #[local]
    struct LocalResources {
        gpiote: Gpiote,
        power: nrf52840_pac::POWER,
    }

    #[monotonic(binds = RTC0, default = true)]
    type RtcMonotonic = nrf52::rtic_monotonic::RtcMonotonic;

    #[init(
        local = [
          store_resources: StoreResources<Board> = StoreResources::new(),
        ]
    )]
    fn init(mut ctx: init::Context) -> (SharedResources, LocalResources, init::Monotonics) {
        let mut init_status = apps::InitStatus::default();

        #[cfg(feature = "alloc")]
        embedded_runner_lib::init_alloc();

        ctx.core.DCB.enable_trace();
        ctx.core.DWT.enable_cycle_counter();

        boards::init::init_logger::<Board>(VERSION_STRING);

        let soc = nrf52::init_bootup(&ctx.device.FICR, &ctx.device.UICR, &mut ctx.device.POWER);

        let mut board_gpio = nk3am::init_pins(ctx.device.GPIOTE, ctx.device.P0, ctx.device.P1);

        let usb_bus = nrf52::setup_usb_bus(ctx.device.CLOCK, ctx.device.USBD);

        let internal_flash = InternalFlashStorage::new(ctx.device.NVMC);
        let external_flash = nk3am::init_external_flash(
            ctx.device.SPIM3,
            board_gpio.flashnfc_spi.take().unwrap(),
            board_gpio.flash_cs.take().unwrap(),
        );
        let store = store::init_store(
            ctx.local.store_resources,
            internal_flash,
            external_flash,
            false,
            &mut init_status,
        );

        static NFC_CHANNEL: CcidChannel = Channel::new();
        let (_nfc_rq, nfc_rp) = NFC_CHANNEL.split().unwrap();
        let usb_nfc = embedded_runner_lib::init_usb_nfc::<Board>(Some(usb_bus), None, nfc_rp);

        #[cfg(feature = "se050")]
        let se050 = nk3am::init_se050(
            ctx.device.TWIM1,
            board_gpio.se_pins.unwrap(),
            board_gpio.se_power.unwrap(),
            ctx.device.TIMER1,
        );

        #[cfg(feature = "board-nk3am")]
        let user_interface = nk3am::init_ui(
            board_gpio.rgb_led,
            ctx.device.PWM0,
            ctx.device.PWM1,
            ctx.device.PWM2,
            board_gpio.touch,
        );

        let mut dev_rng = Rng::new(ctx.device.RNG);
        let hw_key = nk3am::hw_key(&ctx.device.FICR);
        let mut trussed = boards::init::init_trussed(
            &mut dev_rng,
            store,
            user_interface,
            &mut init_status,
            Some(&hw_key),
            #[cfg(feature = "se050")]
            Some(se050),
        );

        let apps = boards::init::init_apps(
            &soc,
            &mut trussed,
            init_status,
            &store,
            false,
            VERSION,
            VERSION_STRING,
        );

        let rtc_mono = RtcMonotonic::new(ctx.device.RTC0);

        ui::spawn_after(RtcDuration::from_ms(2500)).ok();

        (
            SharedResources {
                trussed,
                apps,
                apdu_dispatch: usb_nfc.apdu_dispatch,
                ctaphid_dispatch: usb_nfc.ctaphid_dispatch,
                usb_classes: usb_nfc.usb_classes,
            },
            LocalResources {
                gpiote: board_gpio.gpiote,
                power: ctx.device.POWER,
            },
            init::Monotonics(rtc_mono),
        )
    }

    #[idle(shared = [apps, apdu_dispatch, ctaphid_dispatch, usb_classes])]
    fn idle(ctx: idle::Context) -> ! {
        let idle::SharedResources {
            mut apps,
            mut apdu_dispatch,
            mut ctaphid_dispatch,
            mut usb_classes,
        } = ctx.shared;

        trace!("idle");
        // TODO: figure out whether entering WFI is really worth it
        // cortex_m::asm::wfi();

        loop {
            #[cfg(not(feature = "no-delog"))]
            boards::init::Delogger::flush();

            let (usb_activity, _nfc_activity) = apps.lock(|apps| {
                apdu_dispatch.lock(|apdu_dispatch| {
                    ctaphid_dispatch.lock(|ctaphid_dispatch| {
                        runtime::poll_dispatchers(apdu_dispatch, ctaphid_dispatch, apps)
                    })
                })
            });
            if usb_activity {
                /*trace!("app->usb");*/
                rtic::pend(nrf52840_pac::Interrupt::USBD);
            }

            usb_classes.lock(|usb_classes| {
                runtime::poll_usb(
                    usb_classes,
                    ccid_keepalive::spawn_after,
                    ctaphid_keepalive::spawn_after,
                    monotonics::now().into(),
                );
            });
        }
        // loop {}
    }

    #[task(priority = 2, binds = SWI0_EGU0, shared = [trussed])]
    fn task_trussed(ctx: task_trussed::Context) {
        let mut trussed = ctx.shared.trussed;

        //trace!("irq SWI0_EGU0");
        trussed.lock(|trussed| {
            runtime::run_trussed(trussed);
        });
    }

    #[task(priority = 5, binds = GPIOTE, local = [gpiote])] /* ui, fpr */
    fn task_button_irq(_ctx: task_button_irq::Context) {
        trace!("irq GPIOTE");
    }

    #[task(priority = 3, binds = USBD, shared = [usb_classes])]
    fn task_usb(ctx: task_usb::Context) {
        // trace!("irq USB");
        let mut usb_classes = ctx.shared.usb_classes;

        usb_classes.lock(|usb_classes| {
            runtime::poll_usb(
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
            runtime::ccid_keepalive(usb_classes, ccid_keepalive::spawn_after);
        });
    }

    #[task(priority = 3, shared = [usb_classes])]
    fn ctaphid_keepalive(ctx: ctaphid_keepalive::Context) {
        let mut usb_classes = ctx.shared.usb_classes;

        usb_classes.lock(|usb_classes| {
            runtime::ctaphid_keepalive(usb_classes, ctaphid_keepalive::spawn_after);
        });
    }

    #[task(priority = 5, binds = POWER_CLOCK, local = [power])]
    fn power_handler(ctx: power_handler::Context) {
        nk3am::power_handler(ctx.local.power);
    }

    #[task(priority = 1, shared = [trussed])]
    fn ui(ctx: ui::Context) {
        //trace!("UI");
        let mut trussed = ctx.shared.trussed;

        //trace!("update ui");
        trussed.lock(|trussed| {
            trussed.update_ui();
        });
        ui::spawn_after(RtcDuration::from_ms(125)).ok();
    }
}

#[inline(never)]
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    boards::handle_panic::<boards::nk3am::NK3AM>(info)
}

#[exception]
unsafe fn HardFault(ef: &ExceptionFrame) -> ! {
    boards::handle_hard_fault::<boards::nk3am::NK3AM>(ef)
}
