//! Line-oriented parser for the integration-test DSL.
//!
//! See the crate root for the full grammar. Briefly:
//!
//! ```text
//! sleep <dur>;
//! let <name> = <call>;
//! <call> => <verb> [<operand> | <range>];
//! ```
//!
//! where `<call>` is `<target>.<method>[(<arg>)]` followed by zero or
//! more `.<accessor>` projections, and `<operand>` is a number, `true`,
//! `false`, or a variable name bound by an earlier `let`.
//!
//! SPDX-License-Identifier: MIT

use std::fmt;
use std::time::Duration;

use time_alarm_service_interface::{AcpiTimerId, AcpiTimestamp};

/// Local mirror of [`ec_test_lib::Threshold`] (which is not `Clone`/`Debug`).
#[derive(Debug, Clone, Copy)]
pub enum Threshold {
    On,
    Ramping,
    Max,
}

/// One parsed statement.
#[derive(Debug, Clone)]
pub enum Stmt {
    Check {
        line: usize,
        source: String,
        call: Call,
        verb: Verb,
    },
    Let {
        line: usize,
        source: String,
        name: String,
        call: Call,
    },
    Sleep {
        line: usize,
        duration: Duration,
    },
}

#[derive(Debug, Clone)]
pub struct Call {
    pub target: Target,
    pub method: Method,
    /// Dotted accessor path applied to the method's return value.
    pub projection: Vec<String>,
}

#[derive(Debug, Clone, Copy)]
pub enum Target {
    Thermal,
    Battery,
    Rtc,
}

#[derive(Debug, Clone)]
pub enum Method {
    // Thermal
    GetTemperature,
    GetRpm,
    GetMinRpm,
    GetMaxRpm,
    GetThreshold(Threshold),
    SetThreshold(Threshold, Operand),
    SetRpm(Operand),
    // Battery
    GetBst,
    GetBix,
    SetBtp(Operand),
    // Rtc
    GetCapabilities,
    GetRealTime,
    SetRealTime(AcpiTimestamp),
    GetWakeStatus(AcpiTimerId),
    GetExpiredTimerWakePolicy(AcpiTimerId),
    GetTimerValue(AcpiTimerId),
    SetTimerValue(AcpiTimerId, Operand),
    SetExpiredTimerWakePolicy(AcpiTimerId, Operand),
    ClearWakeStatus(AcpiTimerId),
}

#[derive(Debug, Clone)]
pub enum Operand {
    Num(f64),
    Bool(bool),
    Var(String),
}

#[derive(Debug, Clone)]
pub enum Verb {
    Eq(Operand),
    Ne(Operand),
    Gt(Operand),
    Ge(Operand),
    Lt(Operand),
    Le(Operand),
    InRange { lo: f64, hi: f64, inclusive: bool },
    IsOk,
    IsErr,
    OkEq(Operand),
    OkGt(Operand),
    OkGe(Operand),
    OkLt(Operand),
    OkLe(Operand),
    OkInRange { lo: f64, hi: f64, inclusive: bool },
}

#[derive(Debug)]
pub struct ParseError {
    pub line: usize,
    pub message: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

impl std::error::Error for ParseError {}

pub fn parse(src: &str) -> Result<Vec<Stmt>, ParseError> {
    let mut out = Vec::new();
    let mut buf = String::new();
    let mut start_line = 0usize;

    for (idx, raw) in src.lines().enumerate() {
        let lineno = idx + 1;
        let mut line = raw;
        if let Some(hash) = line.find('#') {
            line = &line[..hash];
        }
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if buf.is_empty() {
            start_line = lineno;
        } else {
            buf.push(' ');
        }
        buf.push_str(line);

        while let Some(semi) = buf.find(';') {
            let stmt_src: String = buf[..semi].trim().to_string();
            buf = buf[semi + 1..].trim_start().to_string();
            if !stmt_src.is_empty() {
                let stmt = parse_stmt(start_line, &stmt_src).map_err(|m| ParseError {
                    line: start_line,
                    message: m,
                })?;
                out.push(stmt);
            }
            start_line = lineno;
        }
    }

    if !buf.trim().is_empty() {
        return Err(ParseError {
            line: start_line,
            message: format!("unterminated statement: `{}` (missing `;`?)", buf.trim()),
        });
    }

    Ok(out)
}

fn parse_stmt(line: usize, src: &str) -> Result<Stmt, String> {
    if let Some(rest) = src.strip_prefix("sleep ") {
        return Ok(Stmt::Sleep {
            line,
            duration: parse_duration(rest.trim())?,
        });
    }

    if let Some(rest) = src.strip_prefix("let ") {
        let (name, expr) = rest
            .split_once('=')
            .ok_or_else(|| format!("`let` needs `name = <call>` (got `{src}`)"))?;
        let name = name.trim().to_string();
        if !is_ident(&name) {
            return Err(format!("invalid variable name `{name}`"));
        }
        let call = parse_call(expr.trim())?;
        return Ok(Stmt::Let {
            line,
            source: src.to_string(),
            name,
            call,
        });
    }

    let (call_src, verb_src) = src
        .split_once("=>")
        .ok_or_else(|| format!("expected `=>` separator in `{src}`"))?;
    let call = parse_call(call_src.trim())?;
    let verb = parse_verb(verb_src.trim())?;
    Ok(Stmt::Check {
        line,
        source: src.to_string(),
        call,
        verb,
    })
}

fn parse_call(src: &str) -> Result<Call, String> {
    // Split at top-level dots (outside parentheses).
    let mut parts: Vec<String> = Vec::new();
    let mut depth = 0usize;
    let mut cur = String::new();
    for ch in src.chars() {
        match ch {
            '(' => {
                depth += 1;
                cur.push(ch);
            }
            ')' => {
                if depth == 0 {
                    return Err(format!("unbalanced parentheses in `{src}`"));
                }
                depth -= 1;
                cur.push(ch);
            }
            '.' if depth == 0 => parts.push(std::mem::take(&mut cur)),
            _ => cur.push(ch),
        }
    }
    parts.push(cur);
    if depth != 0 {
        return Err(format!("unbalanced parentheses in `{src}`"));
    }
    if parts.len() < 2 {
        return Err(format!("expected `<target>.<method>...` in `{src}`"));
    }

    let target = match parts[0].trim() {
        "thermal" => Target::Thermal,
        "battery" => Target::Battery,
        "rtc" => Target::Rtc,
        other => return Err(format!("unknown target `{other}` (expected thermal|battery|rtc)")),
    };

    let (method_name, arg) = split_call(parts[1].trim())?;
    let method = build_method(target, method_name, arg.as_deref())?;

    let projection: Vec<String> = parts[2..]
        .iter()
        .map(|s| {
            let s = s.trim();
            s.strip_suffix("()").unwrap_or(s).to_string()
        })
        .collect();

    for p in &projection {
        // Accept identifiers (`battery_present_voltage`) and pure numeric
        // tuple-struct indices (`0`, `1`).
        if !is_ident(p) && !p.chars().all(|c| c.is_ascii_digit()) {
            return Err(format!("invalid accessor `{p}`"));
        }
    }

    Ok(Call {
        target,
        method,
        projection,
    })
}

fn split_call(s: &str) -> Result<(&str, Option<String>), String> {
    match s.find('(') {
        None => Ok((s, None)),
        Some(open) => {
            let close = s.rfind(')').ok_or_else(|| format!("missing `)` in `{s}`"))?;
            if close < open {
                return Err(format!("malformed call `{s}`"));
            }
            if close + 1 != s.len() {
                return Err(format!("unexpected trailing characters after `)` in `{s}`"));
            }
            let name = s[..open].trim();
            let arg = s[open + 1..close].trim().to_string();
            Ok((name, Some(arg)))
        }
    }
}

fn is_ident(s: &str) -> bool {
    let mut cs = s.chars();
    match cs.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    cs.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn build_method(target: Target, name: &str, arg: Option<&str>) -> Result<Method, String> {
    fn need_no_arg(name: &str, arg: Option<&str>) -> Result<(), String> {
        match arg {
            None | Some("") => Ok(()),
            Some(a) => Err(format!("`{name}` does not take an argument (got `{a}`)")),
        }
    }
    fn need_arg<'a>(name: &str, arg: Option<&'a str>) -> Result<&'a str, String> {
        match arg {
            Some(a) if !a.is_empty() => Ok(a),
            _ => Err(format!("`{name}` requires an argument")),
        }
    }
    match (target, name) {
        (Target::Thermal, "get_temperature") => {
            need_no_arg(name, arg)?;
            Ok(Method::GetTemperature)
        }
        (Target::Thermal, "get_rpm") => {
            need_no_arg(name, arg)?;
            Ok(Method::GetRpm)
        }
        (Target::Thermal, "get_min_rpm") => {
            need_no_arg(name, arg)?;
            Ok(Method::GetMinRpm)
        }
        (Target::Thermal, "get_max_rpm") => {
            need_no_arg(name, arg)?;
            Ok(Method::GetMaxRpm)
        }
        (Target::Thermal, "get_threshold") => {
            let a = need_arg(name, arg)?;
            let th = match a {
                "on" => Threshold::On,
                "ramping" => Threshold::Ramping,
                "max" => Threshold::Max,
                _ => return Err(format!("unknown threshold `{a}` (on|ramping|max)")),
            };
            Ok(Method::GetThreshold(th))
        }
        (Target::Thermal, "set_rpm") => {
            let a = need_arg(name, arg)?;
            Ok(Method::SetRpm(parse_operand(a)?))
        }
        (Target::Thermal, "set_threshold") => {
            let a = need_arg(name, arg)?;
            let (th_str, val_str) = a
                .split_once(',')
                .ok_or_else(|| format!("set_threshold takes `<on|ramping|max>, <celsius>` (got `{a}`)"))?;
            let th = match th_str.trim() {
                "on" => Threshold::On,
                "ramping" => Threshold::Ramping,
                "max" => Threshold::Max,
                _ => return Err(format!("unknown threshold `{th_str}` (on|ramping|max)")),
            };
            Ok(Method::SetThreshold(th, parse_operand(val_str.trim())?))
        }

        (Target::Battery, "get_bst") => {
            need_no_arg(name, arg)?;
            Ok(Method::GetBst)
        }
        (Target::Battery, "get_bix") => {
            need_no_arg(name, arg)?;
            Ok(Method::GetBix)
        }
        (Target::Battery, "set_btp") => {
            let a = need_arg(name, arg)?;
            Ok(Method::SetBtp(parse_operand(a)?))
        }

        (Target::Rtc, "get_capabilities") => {
            need_no_arg(name, arg)?;
            Ok(Method::GetCapabilities)
        }
        (Target::Rtc, "get_real_time") => {
            need_no_arg(name, arg)?;
            Ok(Method::GetRealTime)
        }
        (Target::Rtc, "set_real_time") => Ok(Method::SetRealTime(parse_timestamp_buffer(need_arg(name, arg)?)?)),
        (Target::Rtc, "get_wake_status") => Ok(Method::GetWakeStatus(parse_timer(need_arg(name, arg)?)?)),
        (Target::Rtc, "get_expired_timer_wake_policy") => {
            Ok(Method::GetExpiredTimerWakePolicy(parse_timer(need_arg(name, arg)?)?))
        }
        (Target::Rtc, "get_timer_value") => Ok(Method::GetTimerValue(parse_timer(need_arg(name, arg)?)?)),
        (Target::Rtc, "set_timer_value") => {
            let (id, val) = split_timer_operand(name, need_arg(name, arg)?)?;
            Ok(Method::SetTimerValue(id, val))
        }
        (Target::Rtc, "set_expired_timer_wake_policy") => {
            let (id, val) = split_timer_operand(name, need_arg(name, arg)?)?;
            Ok(Method::SetExpiredTimerWakePolicy(id, val))
        }
        (Target::Rtc, "clear_wake_status") => Ok(Method::ClearWakeStatus(parse_timer(need_arg(name, arg)?)?)),

        (t, n) => Err(format!("unknown method `{n}` on `{}`", target_name(t))),
    }
}

fn target_name(t: Target) -> &'static str {
    match t {
        Target::Thermal => "thermal",
        Target::Battery => "battery",
        Target::Rtc => "rtc",
    }
}

fn parse_timestamp_buffer(s: &str) -> Result<AcpiTimestamp, String> {
    let syntax = || "set_real_time requires `Buffer(16) { <16 explicit bytes> }`".to_string();
    let (size, rest) = s
        .strip_prefix("Buffer")
        .and_then(|s| s.trim_start().strip_prefix('('))
        .and_then(|s| s.split_once(')'))
        .ok_or_else(syntax)?;
    if parse_u8_literal(size)? != 16 {
        return Err("RTC Buffer size must be 16".into());
    }
    let initializer = rest
        .trim()
        .strip_prefix('{')
        .and_then(|s| s.strip_suffix('}'))
        .ok_or_else(syntax)?
        .trim();
    let bytes = initializer
        .strip_suffix(',')
        .unwrap_or(initializer)
        .split(',')
        .map(parse_u8_literal)
        .collect::<Result<Vec<_>, _>>()?;
    let bytes: [u8; 16] = bytes
        .try_into()
        .map_err(|bytes: Vec<u8>| format!("RTC Buffer requires exactly 16 explicit bytes, got {}", bytes.len()))?;
    // Bound milliseconds before the shared decoder multiplies them into u32 nanoseconds.
    let milliseconds = u16::from_le_bytes([bytes[8], bytes[9]]);
    if milliseconds > 999 {
        return Err(format!(
            "invalid RTC timestamp: milliseconds {milliseconds} out of range 0..=999"
        ));
    }
    AcpiTimestamp::try_from_bytes(&bytes).map_err(|e| format!("invalid RTC timestamp: {e:?}"))
}

fn parse_u8_literal(s: &str) -> Result<u8, String> {
    let s = s.trim();
    let (digits, radix) = match s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        Some(hex) => (hex, 16),
        None => (s, 10),
    };
    if digits.is_empty() || !digits.chars().all(|c| c.is_digit(radix)) {
        return Err(format!("expected decimal or hexadecimal byte literal, got `{s}`"));
    }
    u8::from_str_radix(digits, radix).map_err(|e| format!("byte literal `{s}` out of range 0..=255: {e}"))
}

fn parse_timer(s: &str) -> Result<AcpiTimerId, String> {
    match s {
        "ac" => Ok(AcpiTimerId::AcPower),
        "dc" => Ok(AcpiTimerId::DcPower),
        _ => Err(format!("unknown timer `{s}` (ac|dc)")),
    }
}

fn split_timer_operand(name: &str, s: &str) -> Result<(AcpiTimerId, Operand), String> {
    let (id_str, val_str) = s
        .split_once(',')
        .ok_or_else(|| format!("`{name}` takes `<ac|dc>, <u32-or-var>` (got `{s}`)"))?;
    let id = parse_timer(id_str.trim())?;
    Ok((id, parse_operand(val_str.trim())?))
}

fn parse_verb(src: &str) -> Result<Verb, String> {
    let (head, tail) = match src.split_once(char::is_whitespace) {
        Some((h, t)) => (h, t.trim()),
        None => (src, ""),
    };
    let need_operand = |v: &str| -> Result<Operand, String> {
        if tail.is_empty() {
            Err(format!("verb `{v}` requires an operand"))
        } else {
            parse_operand(tail)
        }
    };
    let need_range = |v: &str| -> Result<(f64, f64, bool), String> {
        if tail.is_empty() {
            Err(format!("verb `{v}` requires a range"))
        } else {
            parse_range(tail)
        }
    };
    match head {
        "eq" => Ok(Verb::Eq(need_operand("eq")?)),
        "ne" => Ok(Verb::Ne(need_operand("ne")?)),
        "gt" => Ok(Verb::Gt(need_operand("gt")?)),
        "ge" => Ok(Verb::Ge(need_operand("ge")?)),
        "lt" => Ok(Verb::Lt(need_operand("lt")?)),
        "le" => Ok(Verb::Le(need_operand("le")?)),
        "in_range" => {
            let (lo, hi, inclusive) = need_range("in_range")?;
            Ok(Verb::InRange { lo, hi, inclusive })
        }
        "is_ok" => Ok(Verb::IsOk),
        "is_err" => Ok(Verb::IsErr),
        "ok_eq" => Ok(Verb::OkEq(need_operand("ok_eq")?)),
        "ok_gt" => Ok(Verb::OkGt(need_operand("ok_gt")?)),
        "ok_ge" => Ok(Verb::OkGe(need_operand("ok_ge")?)),
        "ok_lt" => Ok(Verb::OkLt(need_operand("ok_lt")?)),
        "ok_le" => Ok(Verb::OkLe(need_operand("ok_le")?)),
        "ok_in_range" => {
            let (lo, hi, inclusive) = need_range("ok_in_range")?;
            Ok(Verb::OkInRange { lo, hi, inclusive })
        }
        other => Err(format!("unknown verb `{other}`")),
    }
}

fn parse_operand(s: &str) -> Result<Operand, String> {
    let s = s.trim();
    match s {
        "true" => Ok(Operand::Bool(true)),
        "false" => Ok(Operand::Bool(false)),
        _ => {
            if let Ok(n) = s.parse::<f64>() {
                Ok(Operand::Num(n))
            } else if is_ident(s) {
                Ok(Operand::Var(s.to_string()))
            } else {
                Err(format!("expected number/true/false/identifier, got `{s}`"))
            }
        }
    }
}

fn parse_num(s: &str) -> Result<f64, String> {
    s.trim()
        .parse::<f64>()
        .map_err(|e| format!("expected number, got `{s}`: {e}"))
}

fn parse_range(s: &str) -> Result<(f64, f64, bool), String> {
    let s = s.trim();
    if let Some(idx) = s.find("..=") {
        let lo = parse_num(&s[..idx])?;
        let hi = parse_num(&s[idx + 3..])?;
        Ok((lo, hi, true))
    } else if let Some(idx) = s.find("..") {
        let lo = parse_num(&s[..idx])?;
        let hi = parse_num(&s[idx + 2..])?;
        Ok((lo, hi, false))
    } else {
        Err(format!("expected `lo..hi` or `lo..=hi`, got `{s}`"))
    }
}

fn parse_duration(s: &str) -> Result<Duration, String> {
    let s = s.trim();
    let (num_part, unit) = s
        .find(|c: char| c.is_alphabetic())
        .map(|i| (&s[..i], &s[i..]))
        .unwrap_or((s, "ms"));
    let n: u64 = num_part.trim().parse().map_err(|e| format!("duration `{s}`: {e}"))?;
    match unit.trim() {
        "ms" => Ok(Duration::from_millis(n)),
        "s" => Ok(Duration::from_secs(n)),
        "us" => Ok(Duration::from_micros(n)),
        other => Err(format!("unknown duration unit `{other}` (ms|s|us)")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_basic_script() {
        let src = r#"
            thermal.get_temperature => ok_in_range 20.0..=60.0;
            thermal.set_rpm(3000) => is_ok;
            sleep 50ms;
            battery.set_btp(15000) => is_ok;
        "#;
        let stmts = parse(src).unwrap();
        assert_eq!(stmts.len(), 4);
    }

    #[test]
    fn parses_let_projection_and_var_operand() {
        let src = r#"
            let cached = thermal.get_temperature;
            thermal.get_temperature => eq cached;
            battery.get_bst.battery_present_voltage => in_range 11000..=13500;
            rtc.get_capabilities.ac_wake_implemented => eq true;
            rtc.get_capabilities.ac_wake_implemented() => eq true;
        "#;
        let stmts = parse(src).unwrap();
        assert_eq!(stmts.len(), 5);
        match &stmts[0] {
            Stmt::Let { name, .. } => assert_eq!(name, "cached"),
            other => panic!("expected Let, got {other:?}"),
        }
    }

    #[test]
    fn parses_rtc_buffer_literal() {
        for (size, trailing) in [("16", ""), ("0x10", ",")] {
            let src = format!(
                "rtc.set_real_time(Buffer ({size}) {{ # whole seconds
                    0xEA, 0X07, 9, 17, 12, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0{trailing}
                }}) => is_ok;"
            );
            let stmts = parse(&src).unwrap();
            let Stmt::Check {
                call, verb: Verb::IsOk, ..
            } = &stmts[0]
            else {
                panic!("expected RTC setter check");
            };
            let Method::SetRealTime(timestamp) = call.method else {
                panic!("expected SetRealTime");
            };
            let expected =
                AcpiTimestamp::try_from_bytes(&[234, 7, 9, 17, 12, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]).unwrap();
            assert_eq!(timestamp, expected);
        }
    }

    #[test]
    fn rejects_invalid_rtc_buffers() {
        let bytes = "234, 7, 9, 17, 12, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0";
        for arg in [
            String::new(),
            "1760000000".into(),
            "2026, 9, 17, 12, 0, 0, 0, 0, 0".into(),
            format!("Buffer(15) {{ {bytes} }}"),
            format!("Buffer(17) {{ {bytes} }}"),
            format!("Buffer(16.0) {{ {bytes} }}"),
            format!("Buffer(16 + 0) {{ {bytes} }}"),
            "Buffer(16) {}".into(),
            format!("Buffer(16) {{ {} }}", bytes.rsplit_once(',').unwrap().0),
            format!("Buffer(16) {{ {bytes}, 0 }}"),
            format!("Buffer(16) {{ {bytes},, }}"),
            format!("Buffer(16) {{ {} }}", bytes.replacen(", 7", " 7", 1)),
            format!("Buffer(16) {{ {bytes}"),
            format!("Buffer(16) {bytes} }}"),
            format!("Buffer(16 {{ {bytes} }}"),
            format!("Buffer(16)) {{ {bytes} }}"),
            format!("Buffer(16) {{ {bytes} }} garbage"),
            format!("Buffer(16) {{ {bytes} }}, Buffer(16) {{ {bytes} }}"),
        ] {
            let error = parse(&format!("\nrtc.set_real_time({arg}) => is_ok;")).unwrap_err();
            assert_eq!(error.line, 2, "{arg}");
        }
        for byte in [
            "256",
            "0x100",
            "-1",
            "+1",
            "1.5",
            "1e0",
            "0xGG",
            "year",
            "1 + 2",
            "Add(1, 2)",
            "",
        ] {
            let arg = bytes.replacen("234", byte, 1);
            assert!(
                parse(&format!("rtc.set_real_time(Buffer(16) {{ {arg} }}) => is_ok;")).is_err(),
                "{byte}"
            );
        }
    }

    #[test]
    fn rejects_invalid_decoded_timestamps() {
        for (index, byte) in [(2, 13), (3, 32), (4, 24), (5, 60), (6, 60), (8, 232), (11, 6), (12, 2)] {
            let mut bytes = [234u8, 7, 9, 17, 12, 0, 0, 0, 231, 3, 0, 0, 0, 0, 0, 0];
            bytes[index] = byte;
            let initializer = bytes.map(|b| b.to_string()).join(", ");
            let error = parse(&format!("rtc.set_real_time(Buffer(16) {{ {initializer} }}) => is_err;")).unwrap_err();
            assert!(error.message.starts_with("invalid RTC timestamp:"), "{error}");
        }
    }

    #[test]
    fn rejects_milliseconds_that_overflow_decoder_arithmetic() {
        for milliseconds in [4295u16, u16::MAX] {
            let mut bytes = [234u8, 7, 9, 17, 12, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
            bytes[8..10].copy_from_slice(&milliseconds.to_le_bytes());
            let initializer = bytes.map(|b| b.to_string()).join(", ");
            let error = parse(&format!("rtc.set_real_time(Buffer(16) {{ {initializer} }}) => is_ok;")).unwrap_err();
            assert!(error.message.contains("milliseconds"), "{milliseconds}: {error}");
        }
    }
}
