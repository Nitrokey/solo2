#![no_std]
#![no_main]

use embedded_runner_lib as ERL;
use nrf52840_hal::{
	clocks::Clocks,
	gpio::{p0, p1},
	gpiote::Gpiote,
	rng::Rng,
	rtc::Rtc,
	spim::Spim,
};
use panic_halt as _;
use rand_core::SeedableRng;

#[macro_use]
extern crate delog;
delog::generate_macros!();

delog!(Delogger, 3*1024, 512, ERL::types::DelogFlusher);

#[rtic::app(device = nrf52840_hal::pac, peripherals = true, monotonic = rtic::cyccnt::CYCCNT)]
const APP: () = {
        struct Resources {
		trussed: ERL::types::Trussed,
		apps: ERL::types::Apps,
		apdu_dispatch: ERL::types::ApduDispatch,
		ctaphid_dispatch: ERL::types::CtaphidDispatch,
		usb_classes: Option<ERL::types::usbnfc::UsbClasses>,
		contactless: Option<ERL::types::Iso14443>,

		/* NRF specific elements */
		// (display UI)
		// (fingerprint sensor)
		// (SE050)
		/* NRF specific device peripherals */
		gpiote: Gpiote,
		power: nrf52840_pac::POWER,
		rtc: Rtc<nrf52840_pac::RTC0>,

		/* LPC55 specific elements */
		// perf_timer
		// clock_ctrl
		// wait_extender
	}

        #[init()]
        fn init(mut ctx: init::Context) -> init::LateResources {
		ctx.core.DCB.enable_trace();
		ctx.core.DWT.enable_cycle_counter();

		rtt_target::rtt_init_print!();
		Delogger::init_default(delog::LevelFilter::Debug, &ERL::types::DELOG_FLUSHER).ok();
		ERL::banner();

		ERL::soc::init_bootup(&ctx.device.FICR, &ctx.device.UICR, &mut ctx.device.POWER);

		let dev_gpiote = Gpiote::new(ctx.device.GPIOTE);
		let mut board_gpio = {
			let dev_gpio_p0 = p0::Parts::new(ctx.device.P0);
			let dev_gpio_p1 = p1::Parts::new(ctx.device.P1);
			ERL::soc::board::init_pins(&dev_gpiote, dev_gpio_p0, dev_gpio_p1)
		};
		dev_gpiote.reset_events();

		/* check reason for booting */
		let powered_by_usb: bool = true;
		/* a) powered through NFC: enable NFC, keep external oscillator off, don't start USB */
		/* b) powered through USB: start external oscillator, start USB, keep NFC off(?) */

		let usbd_ref = { if powered_by_usb {
			Some(ERL::soc::setup_usb_bus(ctx.device.CLOCK, ctx.device.USBD))
		} else {
			None
		}};
		/* TODO: set up NFC chip */
		let usbnfcinit = ERL::init_usb_nfc(usbd_ref, None);

		let internal_flash = ERL::soc::init_internal_flash(ctx.device.NVMC);
		let external_flash = {
			let dev_spim3 = Spim::new(ctx.device.SPIM3,
				board_gpio.flashnfc_spi.take().unwrap(),
				nrf52840_hal::spim::Frequency::M2,
				nrf52840_hal::spim::MODE_0,
				0x00u8
			);
			ERL::soc::init_external_flash(dev_spim3,
				board_gpio.flash_cs.take().unwrap(),
				board_gpio.flash_power
			)
		};
		let store: ERL::types::RunnerStore = ERL::init_store(internal_flash, external_flash);

		/* TODO: set up fingerprint device */
		/* TODO: set up SE050 device */

		let dev_rng = Rng::new(ctx.device.RNG);
		let chacha_rng = chacha20::ChaCha8Rng::from_rng(dev_rng).unwrap();
		let dummy_ui = ERL::soc::dummy_ui::DummyUI::new();

		let platform: ERL::types::RunnerPlatform = ERL::types::RunnerPlatform::new(
			chacha_rng, store, dummy_ui);

		let mut trussed_service = trussed::service::Service::new(platform);

		let apps = ERL::init_apps(&mut trussed_service, &store, !powered_by_usb);

		let dev_rtc = Rtc::new(ctx.device.RTC0, 4095).unwrap();

		// compose LateResources
		init::LateResources {
			trussed: trussed_service,
			apps,
			apdu_dispatch: usbnfcinit.apdu_dispatch,
			ctaphid_dispatch: usbnfcinit.ctaphid_dispatch,
			usb_classes: usbnfcinit.usb_classes,
			contactless: usbnfcinit.iso14443,
			gpiote: dev_gpiote,
			power: ctx.device.POWER,
			rtc: dev_rtc,
		}
	}

	#[idle(resources = [apps, apdu_dispatch, ctaphid_dispatch, usb_classes])]
	fn idle(ctx: idle::Context) -> ! {
		let idle::Resources { apps, apdu_dispatch, ctaphid_dispatch, mut usb_classes } = ctx.resources;

		trace!("idle");
		// TODO: figure out whether entering WFI is really worth it
		// cortex_m::asm::wfi();

		loop {
			Delogger::flush();

			let _apdu_poll = ERL::poll_apdu(apdu_dispatch, apps);
			let _ctaphid_poll = ERL::poll_ctaphid(ctaphid_dispatch, apps);
			// raise appropriate interrupts

			/// let _usb_poll = usb_classes.lock(|usb_classes_opt| ERL::poll_usb_classes(usb_classes_opt));
			let _usb_poll = ERL::poll_usb_classes(usb_classes);
			// kick off wait extensions if necessary
		}
		// loop {}
	}

};