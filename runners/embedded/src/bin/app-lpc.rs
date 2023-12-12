#![no_std]
#![no_main]

#[macro_use]
extern crate delog;
generate_macros!();

use core::arch::asm;

#[inline]
pub fn msp() -> u32 {
    let r;
    unsafe { asm!("mrs {}, MSP", out(reg) r, options(nomem, nostack, preserves_flags)) };
    r
}

#[rtic::app(device = lpc55_hal::raw, peripherals = true, dispatchers = [PLU, PIN_INT5, PIN_INT7])]
mod app {
    use embedded_runner_lib::{
        runtime,
        soc::{self, monotonic::SystickMonotonic, types::Soc},
        types,
    };
    use lpc55_hal::{
        drivers::timer::Elapsed,
        raw::Interrupt,
        time::{DurationExtensions, Microseconds, Milliseconds},
        traits::wg::timer::{Cancel, CountDown},
    };
    use systick_monotonic::Systick;

    const REFRESH_MILLISECS: Milliseconds = Milliseconds(50);

    const USB_INTERRUPT: Interrupt = Interrupt::USB1;
    const NFC_INTERRUPT: Interrupt = Interrupt::PIN_INT0;

    #[shared]
    struct SharedResources {
        /// Dispatches APDUs from contact+contactless interface to apps.
        apdu_dispatch: types::ApduDispatch,

        /// Dispatches CTAPHID messages to apps.
        ctaphid_dispatch: types::CtaphidDispatch,

        /// The Trussed service, used by all applications.
        trussed: types::Trussed,

        /// All the applications that the device serves.
        apps: types::Apps,

        /// The USB driver classes
        usb_classes: Option<types::usbnfc::UsbClasses<Soc>>,
        /// The NFC driver
        contactless: Option<types::Iso14443>,

        /// This timer is used while developing NFC, to time how long things took,
        /// and to make sure logs are not flushed in the middle of NFC transactions.
        ///
        /// It could and should be behind some kind of `debug-nfc-timer` feature flag.
        perf_timer: soc::types::PerformanceTimer,

        /// When using passive power (i.e. NFC), we switch between 12MHz
        /// and 48Mhz, trying to optimize speed while keeping power high enough.
        ///
        /// In principle, we could just run at 12MHz constantly, and then
        /// there would be no need for a system-speed independent wait extender.
        clock_ctrl: Option<soc::types::DynamicClockController>,

        /// Applications must respond to NFC requests within a certain time frame (~40ms)
        /// or send a "wait extension" to the NFC reader. This timer is responsible
        /// for scheduling these.
        ///
        /// In the current version of RTIC, the built-in scheduling cannot be used, as it
        /// is expressed in terms of cycles, and our dynamic clock control potentially changes
        /// timing. It seems like RTIC v6 will allow using such a timer directly.
        ///
        /// Alternatively, we could send wait extensions as if always running at 12MHz,
        /// which would cause more context switching and NFC exchangs though.
        ///
        /// NB: CCID + CTAPHID also have a sort of "wait extension" implemented, however
        /// since the system runs at constant speed when powered over USB, there is no
        /// need for such an independent timer.
        wait_extender: soc::types::NfcWaitExtender,
    }

    #[local]
    struct LocalResources {}

    // TODO: replace
    #[monotonic(binds = SysTick, default = true)]
    type Monotonic = SystickMonotonic;

    #[init()]
    fn init(c: init::Context) -> (SharedResources, LocalResources, init::Monotonics) {
        #[cfg(feature = "alloc")]
        embedded_runner_lib::init_alloc();

        let soc::init::All {
            basic,
            usb_nfc,
            trussed,
            apps,
            clock_controller,
        } = soc::init(c.device, c.core);
        let perf_timer = basic.perf_timer;
        let wait_extender = basic.delay_timer;

        // don't toggle LED in passive mode
        if usb_nfc.usb_classes.is_some() {
            lpc55_hal::enable_cycle_counter();
            update_ui::spawn_after(REFRESH_MILLISECS).ok();
        }

        let systick = unsafe { lpc55_hal::raw::CorePeripherals::steal() }.SYST;
        let systick = Systick::new(systick, 96_000_000); // TODO: read out sysclk

        let shared = SharedResources {
            apdu_dispatch: usb_nfc.apdu_dispatch,
            ctaphid_dispatch: usb_nfc.ctaphid_dispatch,
            trussed,
            apps,
            usb_classes: usb_nfc.usb_classes,
            contactless: usb_nfc.iso14443,
            perf_timer,
            clock_ctrl: clock_controller,
            wait_extender,
        };
        (shared, LocalResources {}, init::Monotonics(systick.into()))
    }

    #[idle(shared = [apdu_dispatch, ctaphid_dispatch, apps, perf_timer, usb_classes])]
    fn idle(c: idle::Context) -> ! {
        let idle::SharedResources {
            mut apdu_dispatch,
            mut ctaphid_dispatch,
            mut apps,
            mut perf_timer,
            mut usb_classes,
        } = c.shared;

        info_now!("inside IDLE, initial SP = {:08X}", super::msp());
        loop {
            let mut time = 0;
            perf_timer.lock(|perf_timer| {
                time = perf_timer.elapsed().0;
                if time == 60_000_000 {
                    perf_timer.start(60_000_000.microseconds());
                }
            });

            #[cfg(not(feature = "no-delog"))]
            if time > 1_200_000 {
                soc::Delogger::flush();
            }

            let (usb_activity, nfc_activity) = apps.lock(|apps| {
                apdu_dispatch.lock(|apdu_dispatch| {
                    ctaphid_dispatch.lock(|ctaphid_dispatch| {
                        runtime::poll_dispatchers(apdu_dispatch, ctaphid_dispatch, apps)
                    })
                })
            });
            if usb_activity {
                rtic::pend(USB_INTERRUPT);
            }
            if nfc_activity {
                rtic::pend(NFC_INTERRUPT);
            }

            usb_classes.lock(|usb_classes| {
                runtime::poll_usb(
                    usb_classes,
                    ccid_wait_extension::spawn_after,
                    ctaphid_keepalive::spawn_after,
                    monotonics::now(),
                );
            });

            // TODO: re-enable?
            /*
            contactless.lock(|contactless| {
                runtime::poll_nfc(contactless, nfc_keepalive::spawn_after);
            });
            */
        }
    }

    #[task(binds = USB1_NEEDCLK, shared = [], priority=6)]
    fn usb1_needclk(_c: usb1_needclk::Context) {
        // Behavior is same as in USB1 handler
        rtic::pend(USB_INTERRUPT);
    }

    /// Manages all traffic on the USB bus.
    #[task(binds = USB1, shared = [usb_classes], priority=6)]
    fn usb(mut c: usb::Context) {
        // let remaining = super::msp() - 0x2000_0000;
        // if remaining < 100_000 {
        //     debug_now!("USB interrupt: remaining stack size: {} bytes", remaining);
        // }
        let usb = unsafe { lpc55_hal::raw::Peripherals::steal().USB1 };
        // let before = Instant::now();
        c.shared.usb_classes.lock(|usb_classes| {
            runtime::poll_usb(
                usb_classes,
                ccid_wait_extension::spawn_after,
                ctaphid_keepalive::spawn_after,
                monotonics::now(),
            );
        });

        // let after = Instant::now();
        // let length = (after - before).as_cycles();
        // if length > 10_000 {
        //     // debug!("poll took {:?} cycles", length);
        // }
        let inten = usb.inten.read().bits();
        let intstat = usb.intstat.read().bits();
        let mask = inten & intstat;
        if mask != 0 {
            for i in 0..5 {
                if mask & (1 << 2 * i) != 0 {
                    // debug!("EP{}OUT", i);
                }
                if mask & (1 << (2 * i + 1)) != 0 {
                    // debug!("EP{}IN", i);
                }
            }
            // Serial sends a stray 0x70 ("p") to CDC-ACM "data" OUT endpoint (3)
            // Need to fix that at the management, for now just clear that interrupt.
            usb.intstat.write(|w| unsafe { w.bits(64) });
            // usb.intstat.write(|w| unsafe{ w.bits( usb.intstat.read().bits() ) });
        }

        // if remaining < 60_000 {
        //     debug_now!("USB interrupt done: {} bytes", remaining);
        // }
    }

    /// Whenever we start waiting for an application to reply to CCID, this must be scheduled.
    /// In case the application takes too long, this will periodically send wait extensions
    /// until the application replied.
    #[task(shared = [usb_classes], priority = 6)]
    fn ccid_wait_extension(mut c: ccid_wait_extension::Context) {
        debug_now!("CCID WAIT EXTENSION");
        debug_now!("remaining stack size: {} bytes", super::msp() - 0x2000_0000);
        c.shared.usb_classes.lock(|usb_classes| {
            runtime::ccid_keepalive(usb_classes, ccid_wait_extension::spawn_after)
        });
    }

    /// Same as with CCID, but sending ctaphid keepalive statuses.
    #[task(shared = [usb_classes], priority = 6)]
    fn ctaphid_keepalive(mut c: ctaphid_keepalive::Context) {
        debug_now!("CTAPHID keepalive");
        debug_now!("remaining stack size: {} bytes", super::msp() - 0x2000_0000);
        c.shared.usb_classes.lock(|usb_classes| {
            runtime::ctaphid_keepalive(usb_classes, ctaphid_keepalive::spawn_after)
        });
    }

    #[task(binds = MAILBOX, shared = [usb_classes], priority = 5)]
    #[allow(unused_mut, unused_variables)]
    fn mailbox(mut c: mailbox::Context) {
        // debug_now!("mailbox: remaining stack size: {} bytes", super::msp() - 0x2000_0000);
        #[cfg(feature = "log-serial")]
        c.shared.usb_classes.lock(|usb_classes_maybe| {
            match usb_classes_maybe.as_mut() {
                Some(usb_classes) => {
                    // usb_classes.serial.write(logs.as_bytes()).ok();
                    usb_classes.serial.write(b"dummy test string\n").ok();
                    // app::drain_log_to_serial(&mut usb_classes.serial);
                }
                _ => {}
            }
        });
        // // let usb_classes = c.shared.usb_classes.as_mut().unwrap();

        // let mailbox::Resources { usb_classes } = c.shared;
        // let x: () = usb_classes;
        // // if let Some(usb_classes) = usb_classes.as_mut() {
        // //     usb_classes.serial.write(b"dummy test string\n").ok();
        // // }
    }

    #[task(binds = OS_EVENT, shared = [trussed], priority = 5)]
    fn os_event(mut c: os_event::Context) {
        // debug_now!("os event: remaining stack size: {} bytes", super::msp() - 0x2000_0000);
        c.shared
            .trussed
            .lock(|trussed| runtime::run_trussed(trussed));
    }

    #[task(shared = [trussed], priority = 1)]
    fn update_ui(mut c: update_ui::Context) {
        // debug_now!("update UI: remaining stack size: {} bytes", super::msp() - 0x2000_0000);

        c.shared.trussed.lock(|trussed| trussed.update_ui());
        update_ui::spawn_after(REFRESH_MILLISECS).ok();
    }

    #[task(binds = CTIMER0, shared = [contactless, perf_timer, wait_extender], priority = 7)]
    fn nfc_wait_extension(mut c: nfc_wait_extension::Context) {
        c.shared.contactless.lock(|contactless| {
            if let Some(contactless) = contactless.as_mut() {
                c.shared.wait_extender.lock(|wait_extender| {
                    c.shared.perf_timer.lock(|_perf_timer| {
                        // clear the interrupt
                        wait_extender.cancel().ok();

                        info!("<{}", _perf_timer.elapsed().0 / 100);
                        let status = contactless.poll_wait_extensions();
                        match status {
                            nfc_device::Iso14443Status::Idle => {}
                            nfc_device::Iso14443Status::ReceivedData(milliseconds) => {
                                wait_extender.start(Microseconds::try_from(milliseconds).unwrap());
                            }
                        }
                        info!(" {}>", _perf_timer.elapsed().0 / 100);
                    });
                });
            }
        });
    }

    #[task(binds = PIN_INT0, shared = [contactless, perf_timer, wait_extender], priority = 7)]
    fn nfc_irq(mut c: nfc_irq::Context) {
        c.shared.contactless.lock(|contactless| {
            c.shared.perf_timer.lock(|perf_timer| {
                c.shared.wait_extender.lock(|wait_extender| {
                    let contactless = contactless.as_mut().unwrap();
                    let _starttime = perf_timer.elapsed().0 / 100;

                    info!("[");
                    let status = contactless.poll();
                    match status {
                        nfc_device::Iso14443Status::Idle => {}
                        nfc_device::Iso14443Status::ReceivedData(milliseconds) => {
                            wait_extender.cancel().ok();
                            wait_extender.start(Microseconds::try_from(milliseconds).unwrap());
                        }
                    }
                    info!("{}-{}]", _starttime, perf_timer.elapsed().0 / 100);

                    perf_timer.cancel().ok();
                    perf_timer.start(60_000_000.microseconds());
                });
            });
        });
    }

    #[task(binds = ADC0, shared = [clock_ctrl], priority = 8)]
    fn adc_int(mut c: adc_int::Context) {
        c.shared
            .clock_ctrl
            .lock(|clock_ctrl| clock_ctrl.as_mut().unwrap().handle());
    }
}
