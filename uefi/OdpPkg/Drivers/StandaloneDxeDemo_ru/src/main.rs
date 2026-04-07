// no_std: Disables Rust's standard library (similar to how UEFI drivers don't link against a C runtime).
// no_main: Disables the default Rust entry point. The driver uses efi_main as its entry point instead,
//          matching the UEFI driver model.
#![no_std]
#![no_main]

// core::fmt::Write       - Trait that enables formatted string writing (like Sprintf).
// core::panic::PanicInfo - Carries context about a panic (message, source file, line number).
//                          Used by the #[panic_handler] below, similar to ASSERT() debug info.
// log::{...}             - Rust's logging framework, similar to DebugLib's DEBUG() macro.
// r_efi                  - Provides UEFI type definitions (Status, SystemTable, etc.) equivalent to the EDK II headers.
// spin::Mutex            - A spinlock-based mutex. In no_std environments there's no OS thread scheduler,
//                          so spin locks are used instead of blocking locks.
// uart_16550             - Crate that provides 16550 UART register access, similar to SerialPortLib.
use core::{fmt::Write, panic::PanicInfo};
use log::{info, Level, LevelFilter, Metadata, Record};
use r_efi::efi::Status;
use spin::Mutex;
use uart_16550::SerialPort;

// DebugLogger is a struct (similar to a C struct) that wraps a UART serial port behind a Mutex.
// In Rust, shared mutable state must be explicitly synchronized. The Mutex ensures only one caller
// writes to the UART at a time, enforced at compile time by the borrow checker.
struct DebugLogger {
    uart: Mutex<SerialPort>,
}

// `impl` defines methods on a struct, similar to class member functions in C++.
// `const fn` means this function can be evaluated at compile time, enabling static initialization
//    without a constructor/entry-point call — equivalent to a CONST PCDs initializer.
// `unsafe` is required because raw hardware I/O port access (0x402) cannot be verified by the compiler.
//    Change the port address to match your platform's debug UART base address.
impl DebugLogger {
    const fn new() -> Self {
        Self {
            uart: Mutex::new(unsafe { SerialPort::new(0x402) }),
        }
    }
}

// Implements Rust's log::Log trait for DebugLogger, making it a drop-in logging backend.
// Traits are similar to UEFI protocols — they define an interface that a struct must implement.
// Once registered, any code in this driver can use the log macros (info!, warn!, error!, etc.)
// and output will be routed through this implementation to the UART.
impl log::Log for DebugLogger {
    // Returns true if the log level provided by metadata.level() is Info or lower (Info, Warn, Error).
    // Equivalent to checking PcdDebugPrintErrorLevel before calling SerialPortWrite.
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= Level::Info
    }

    // Formats and writes a log message to the serial port.
    // heapless::String<256> is a fixed-size stack-allocated string buffer (no heap allocation),
    // similar to a CHAR8 Buffer[256] on the stack in C.
    fn log(&self, record: &Record) {
        if self.enabled(record.metadata()) {
            let mut buffer = heapless::String::<256>::new();

            // write!() is Rust's formatted print macro, similar to AsciiSPrint().
            // The underscore (_) discards the Result — if the buffer is too small, output is truncated.
            let _ = write!(buffer, "[{:5}] {}\r\n", record.level(), record.args());

            // .lock() acquires the spinlock and returns a guard that auto-releases when it goes out of scope.
            // This is Rust's RAII pattern — no need to manually call ReleaseLock().
            let mut uart = self.uart.lock();
            for byte in buffer.as_bytes() {
                uart.send(*byte);
            }
        }
    }

    // No-op: the UART is synchronous so there's nothing to flush (no output buffer to drain).
    fn flush(&self) {}
}

// `static` creates a global variable with a fixed memory address, similar to a global in C.
// Rust statics must be thread-safe; the Mutex inside DebugLogger satisfies this requirement.
static LOGGER: DebugLogger = DebugLogger::new();

// #[no_mangle] prevents the Rust compiler from renaming this symbol, so the linker can find it.
// `extern "efiapi"` specifies the UEFI calling convention (MS x64 ABI on x86_64).
// This is equivalent to:
//   EFI_STATUS EFIAPI EfiMain(EFI_HANDLE ImageHandle, EFI_SYSTEM_TABLE *SystemTable)
#[no_mangle]
pub extern "efiapi" fn efi_main(
    _image_handle: *const core::ffi::c_void,
    _system_table: *const r_efi::system::SystemTable,
) -> u64 {
    // Register the global logger. set_logger() returns a Result; .map() applies the closure
    // only on success. The leading underscore discards the Result (logger registration only
    // fails if a logger was already set).
    let _ = log::set_logger(&LOGGER).map(|()| log::set_max_level(LevelFilter::Info));

    // This log message will be sent to the UART via our DebugLogger implementation. The log level is Info,
    // so it will be printed because our enabled() method returns true for Info and below.
    info!("Hello Rust UART DXE Demo!");

    // NOTE:
    // This is where the user would normally install protocols, create events, or perform other driver initialization tasks.

    // Status::SUCCESS is the r-efi equivalent of EFI_SUCCESS. The cast chain converts
    // the EFI_STATUS (usize) to u64 to match the return type.
    Status::SUCCESS.as_usize() as u64
}

// Required in no_std: Rust needs a panic handler to know what to do on unrecoverable errors
// (like an assert failure). This infinite loop is equivalent to CpuDeadLoop() in EDK II.
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {}
}
