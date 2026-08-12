//! CLI EC Test tool `eval` subcommand — raw ACPI method evaluation.
//!
//! SPDX-License-Identifier: MIT
//!

use crate::cli::EvalCommand;
use ec_test_lib::acpi::{Acpi, AcpiMethodArgument};
use std::num::ParseIntError;

/// Parse a single CLI token into an ACPI method argument.
///
/// Mirrors the legacy C++ `ectest` parser: `{guid}`, `'string'`, or integer
/// (decimal, `0x` hex, or `0b` binary, optionally negated).
fn parse_arg(token: &str) -> Result<AcpiMethodArgument, String> {
    if let Some(inner) = token.strip_prefix('{').and_then(|t| t.strip_suffix('}')) {
        let uuid = uuid::Uuid::parse_str(inner).map_err(|e| format!("invalid GUID `{token}`: {e}"))?;
        Ok(AcpiMethodArgument::Guid(uuid.to_bytes_le()))
    } else if let Some(inner) = token.strip_prefix('\'').and_then(|t| t.strip_suffix('\'')) {
        Ok(AcpiMethodArgument::String(inner.to_string()))
    } else {
        let value = parse_int(token).map_err(|e| format!("invalid integer `{token}`: {e}"))?;
        Ok(AcpiMethodArgument::Int(value))
    }
}

/// Parse an integer token as a 32-bit ACPI value; negatives use two's complement.
fn parse_int(token: &str) -> Result<u32, ParseIntError> {
    let (negative, body) = match token.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, token),
    };
    let lower = body.to_ascii_lowercase();
    let magnitude = if let Some(hex) = lower.strip_prefix("0x") {
        u32::from_str_radix(hex, 16)
    } else if let Some(bin) = lower.strip_prefix("0b") {
        u32::from_str_radix(bin, 2)
    } else {
        body.parse::<u32>()
    }?;

    Ok(if negative { magnitude.wrapping_neg() } else { magnitude })
}

pub fn run(source: &Acpi, cmd: &EvalCommand) -> Result<(), Box<dyn std::error::Error>> {
    // ACPI permits at most 7 method arguments.
    if cmd.args.len() > 7 {
        return Err("ACPI methods accept at most 7 arguments".into());
    }

    let parsed = cmd.args.iter().map(|a| parse_arg(a)).collect::<Result<Vec<_>, _>>()?;

    let results = source.eval(&cmd.method, &parsed)?;
    if results.is_empty() {
        println!("{} returned no values", cmd.method);
    } else {
        println!("{} returned {} value(s):", cmd.method, results.len());
        for (i, value) in results.iter().enumerate() {
            println!("  [{i}] {value}");
        }
    }
    Ok(())
}
