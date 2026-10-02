//! Text-DSL integration test runner.
//!
//! Test files are line-oriented and human-readable. The verb
//! vocabulary mirrors the on-target self-test DSL; predicate
//! implementations are inlined in the runner.
//!
//! # Statement forms
//!
//! ```text
//! # comments start with '#'
//! sleep <duration>;             # e.g. 100ms, 2s
//! let <name> = <call>;          # bind the result of a call
//! <call> => <verb> [<operand> | <range>];
//! ```
//!
//! ## Calls and projections
//!
//! A `<call>` is `<target>.<method>[(<arg>)]` optionally followed by
//! one or more `.<accessor>` projections:
//!
//! * `thermal.get_temperature`
//! * `battery.get_bst.battery_present_voltage`
//! * `battery.get_bix.cycle_count`
//! * `rtc.get_capabilities.ac_wake_implemented`
//! * `rtc.get_real_time.datetime.unix_timestamp`
//!
//! Bitfield accessors may optionally be written with `()` (e.g.
//! `ac_wake_implemented()`); both forms are equivalent.
//!
//! `rtc.set_real_time(Buffer(16) { <bytes> })` accepts exactly 16 explicit
//! decimal/hexadecimal bytes, with an optional trailing comma; size `0x10`
//! is also accepted. This is a literal, not ASL evaluation or zero filling.
//! Milliseconds are checked before decoding to prevent nanosecond overflow.
//! Validation otherwise reuses `AcpiTimestamp::try_from_bytes`: milliseconds `0..=999`,
//! daylight `0`/`1`/`3`, and byte 7 acceptance are existing compatibility
//! choices, not normative ACPI guarantees. There is no normalization;
//! the source receives the decoded timestamp. See the README for details.
//!
//! ## Operands
//!
//! Verb operands can be:
//! * a number literal: `eq 0`, `ok_in_range 1..=10`
//! * a bool literal: `eq true`, `eq false`
//! * an identifier bound by an earlier `let`: `eq cached`
//!
//! # Example
//!
//! ```text
//! let cached = thermal.get_temperature;
//! sleep 200ms;
//! thermal.get_temperature                   => ne cached;
//! battery.get_bst.battery_present_voltage   => in_range 1000..=30000;
//! rtc.get_capabilities.realtime_implemented => eq true;
//! rtc.get_real_time.datetime.unix_timestamp => gt 0;
//! ```
//!
//! SPDX-License-Identifier: MIT

mod exec;
mod parser;
mod runner;
mod value;
mod verbs;

pub use exec::{Env, execute};
pub use parser::{ParseError, Stmt, parse};
pub use runner::{Outcome, Runner, Summary};
pub use value::Value;

use ec_test_lib::Source;

/// Parse `script` and run every statement against `source`, returning a
/// [`Summary`] of pass/fail counts. A failing row does not abort the run;
/// every statement is executed.
pub fn run_script<S: Source>(source: &S, script: &str) -> Result<Summary, ParseError> {
    let stmts = parse(script)?;
    let mut runner = Runner::new();
    let mut env = Env::new();
    for stmt in &stmts {
        execute(source, stmt, &mut env, &mut runner);
    }
    Ok(runner.into_summary())
}
