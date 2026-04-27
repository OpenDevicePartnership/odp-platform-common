//! Entry-point macro for TianoCore DXE drivers.
//!
//! ## Usage
//!
//! The macro generates the global allocator, `efi_main` entry point, and
//! panic handler. The entire driver file is:
//!
//! ```rust,ignore
//! #![no_std]
//! #![no_main]
//! #![feature(allocator_api)]
//!
//! patina_tianocore::driver_entry!(platform: my_platform::MyPlatform);
//! ```
//!
//! For drivers that need custom logic before/after dispatch, use the
//! function form:
//!
//! ```rust,ignore
//! #![no_std]
//! #![no_main]
//! #![feature(allocator_api)]
//!
//! patina_tianocore::driver_entry!(driver_main);
//!
//! fn driver_main(mut ctx: DriverContext) -> DriverResult {
//!     // Custom pre-dispatch logic...
//!     ctx.dispatch_platform::<MyPlatform>()?;
//!     // Custom post-dispatch logic...
//!     Ok(())
//! }
//! ```

/// Generates the TianoCore `efi_main` entry point, global allocator, and panic handler.
///
/// Accepts either:
///
/// - **A [`Platform`](crate::Platform) type** — auto-dispatches all registered components:
///   ```rust,ignore
///   patina_tianocore::driver_entry!(platform: MyPlatform);
///   ```
///
/// - **A function** `fn(DriverContext) -> DriverResult` — for custom entry logic:
///   ```rust,ignore
///   patina_tianocore::driver_entry!(my_entry_fn);
///   ```
#[macro_export]
macro_rules! driver_entry {
    // Form 1: Platform type — auto-dispatch.
    (platform: $platform:ty) => {
        $crate::__driver_entry_shell!(|mut ctx: $crate::DriverContext| ctx.dispatch_platform::<$platform>());
    };

    // Form 2: Custom entry function `fn(DriverContext) -> DriverResult`.
    ($entry_fn:path) => {
        $crate::__driver_entry_shell!($entry_fn);
    };
}

/// Internal shared expansion for [`driver_entry!`]. Not part of the public API.
///
/// Both forms of `driver_entry!` reduce to a single call here, parameterised
/// by the dispatch expression. Keeping the boilerplate in one place means
/// the allocator, panic handler, and `Status` conversion can't drift between
/// the two forms.
#[doc(hidden)]
#[macro_export]
macro_rules! __driver_entry_shell {
    ($run:expr) => {
        #[global_allocator]
        static ALLOCATOR: $crate::boot_services::UefiAllocator = $crate::boot_services::UefiAllocator::new();

        #[unsafe(no_mangle)]
        pub extern "efiapi" fn efi_main(
            image_handle: *const core::ffi::c_void,
            system_table: *const $crate::macro_support::SystemTable,
        ) -> u64 {
            unsafe {
                ALLOCATOR.init((*system_table).boot_services);
                $crate::logger::init(system_table);
            }

            let ctx = unsafe { $crate::DriverContext::from_system_table(image_handle, system_table) };

            match ($run)(ctx) {
                Ok(()) => $crate::macro_support::Status::SUCCESS.as_usize() as u64,
                Err(e) => {
                    let status: $crate::macro_support::Status = e.into();
                    status.as_usize() as u64
                }
            }
        }

        #[cfg(not(test))]
        #[panic_handler]
        fn panic(info: &core::panic::PanicInfo) -> ! {
            // Surface the panic message before parking. By the time any
            // driver-execution panic occurs the logger is initialised by
            // `efi_main` above; pre-init panics fall through to the `log`
            // crate's no-op behaviour so this call is safe unconditionally.
            log::error!("driver panic: {}", info);
            // Park the CPU. `spin_loop` lowers to PAUSE on x86 and YIELD on
            // aarch64, so we don't burn power in a tight loop while the
            // firmware decides what to do with the panicked driver.
            loop {
                core::hint::spin_loop();
            }
        }
    };
}
