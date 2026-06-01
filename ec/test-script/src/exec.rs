//! Statement executor: dispatch a parsed [`Stmt`] against a [`Source`].
//!
//! Method returns are converted to [`Value`]s (numbers, bools, structs)
//! so the same dotted-path syntax can reach individual battery/BIX
//! fields, bitfield accessors on `TimeAlarmDeviceCapabilities` /
//! `TimerStatus`, and the timestamp wrapped inside `AcpiTimestamp`.
//!
//! SPDX-License-Identifier: MIT

use std::collections::HashMap;

use crate::parser::{Call, Method, Operand, Stmt, Threshold, Verb};
use crate::runner::{Outcome, Runner};
use crate::value::Value;
use crate::verbs;
use battery_service_messages::{BixFixedStrings, BstReturn};
use ec_test_lib::{BatterySource, RtcSource, Source, ThermalSource};
use time_alarm_service_messages::{
    AcpiTimestamp, AlarmExpiredWakePolicy, AlarmTimerSeconds, TimeAlarmDeviceCapabilities, TimerStatus,
};

/// Variable bindings populated by `let` statements.
pub type Env = HashMap<String, Value>;

pub fn execute<S: Source>(source: &S, stmt: &Stmt, env: &mut Env, runner: &mut Runner) {
    match stmt {
        Stmt::Sleep { duration, .. } => std::thread::sleep(*duration),

        Stmt::Let {
            line,
            source: src,
            name,
            call,
        } => {
            let result = eval_call(source, call, env);
            match result {
                Ok(v) => {
                    let display = format!("{v}");
                    env.insert(name.clone(), v);
                    runner.record(*line, src, Outcome::Pass, &format!("let {name} = {display}"));
                }
                Err(e) => {
                    runner.record(*line, src, Outcome::Fail, &format!("let {name}: {e}"));
                }
            }
        }

        Stmt::Check {
            line,
            source: src,
            call,
            verb,
        } => {
            let result = eval_call(source, call, env);
            let (ok, detail) = apply_verb(verb, &result, env);
            let outcome = if ok { Outcome::Pass } else { Outcome::Fail };
            runner.record(*line, src, outcome, &detail);
        }
    }
}

fn eval_call<S: Source>(s: &S, call: &Call, env: &Env) -> Result<Value, String> {
    let v = invoke(s, &call.method, env)?;
    let projected = v.project(&call.projection)?;
    Ok(projected.clone())
}

// ── Method dispatch ─────────────────────────────────────────────────────────

fn invoke<S: Source>(s: &S, m: &Method, env: &Env) -> Result<Value, String> {
    fn err<E: ec_test_lib::Error>(e: E) -> String {
        format!("{e} ({:?})", e.kind())
    }

    // Resolve an Operand used in a setter argument.
    fn num_arg(op: &Operand, env: &Env) -> Result<f64, String> {
        match op {
            Operand::Num(n) => Ok(*n),
            Operand::Bool(_) => Err("setter argument: expected number, got bool".into()),
            Operand::Var(name) => env
                .get(name)
                .ok_or_else(|| format!("undefined variable `{name}`"))?
                .as_num(),
        }
    }
    fn u32_arg(op: &Operand, env: &Env) -> Result<u32, String> {
        let n = num_arg(op, env)?;
        if !n.is_finite() || n < 0.0 || n > u32::MAX as f64 {
            return Err(format!("setter argument: {n} out of u32 range"));
        }
        Ok(n as u32)
    }

    match m {
        Method::GetTemperature => ThermalSource::get_temperature(s).map(Value::Num).map_err(err),
        Method::GetRpm => ThermalSource::get_rpm(s).map(Value::Num).map_err(err),
        Method::GetMinRpm => ThermalSource::get_min_rpm(s).map(Value::Num).map_err(err),
        Method::GetMaxRpm => ThermalSource::get_max_rpm(s).map(Value::Num).map_err(err),
        Method::GetThreshold(t) => ThermalSource::get_threshold(
            s,
            match t {
                Threshold::On => ec_test_lib::Threshold::On,
                Threshold::Ramping => ec_test_lib::Threshold::Ramping,
                Threshold::Max => ec_test_lib::Threshold::Max,
            },
        )
        .map(Value::Num)
        .map_err(err),
        Method::SetRpm(rpm) => ThermalSource::set_rpm(s, num_arg(rpm, env)?)
            .map(|()| Value::Unit)
            .map_err(err),
        Method::SetThreshold(t, v) => ThermalSource::set_threshold(
            s,
            match t {
                Threshold::On => ec_test_lib::Threshold::On,
                Threshold::Ramping => ec_test_lib::Threshold::Ramping,
                Threshold::Max => ec_test_lib::Threshold::Max,
            },
            num_arg(v, env)?,
        )
        .map(|()| Value::Unit)
        .map_err(err),

        Method::GetBst => BatterySource::get_bst(s).map(bst_to_value).map_err(err),
        Method::GetBix => BatterySource::get_bix(s).map(bix_to_value).map_err(err),
        Method::SetBtp(v) => BatterySource::set_btp(s, u32_arg(v, env)?)
            .map(|()| Value::Unit)
            .map_err(err),

        Method::GetCapabilities => RtcSource::get_capabilities(s).map(caps_to_value).map_err(err),
        Method::GetRealTime => RtcSource::get_real_time(s).map(timestamp_to_value).map_err(err),
        Method::GetWakeStatus(id) => RtcSource::get_wake_status(s, *id)
            .map(wake_status_to_value)
            .map_err(err),
        Method::GetExpiredTimerWakePolicy(id) => RtcSource::get_expired_timer_wake_policy(s, *id)
            .map(policy_to_value)
            .map_err(err),
        Method::GetTimerValue(id) => RtcSource::get_timer_value(s, *id)
            .map(timer_value_to_value)
            .map_err(err),
        Method::SetTimerValue(id, v) => RtcSource::set_timer_value(s, *id, AlarmTimerSeconds(u32_arg(v, env)?))
            .map(|()| Value::Unit)
            .map_err(err),
        Method::SetExpiredTimerWakePolicy(id, v) => {
            RtcSource::set_expired_timer_wake_policy(s, *id, AlarmExpiredWakePolicy(u32_arg(v, env)?))
                .map(|()| Value::Unit)
                .map_err(err)
        }
        Method::ClearWakeStatus(id) => RtcSource::clear_wake_status(s, *id).map(|()| Value::Unit).map_err(err),
    }
}

// ── Value conversions for structured returns ───────────────────────────────

fn bst_to_value(b: BstReturn) -> Value {
    // `battery_state` is a custom type without a trivial numeric
    // representation, so it's omitted; project to the u32 fields
    // (`battery_present_voltage`, etc.) for verb checks.
    Value::Struct(vec![
        ("battery_present_rate".into(), Value::Num(b.battery_present_rate as f64)),
        (
            "battery_remaining_capacity".into(),
            Value::Num(b.battery_remaining_capacity as f64),
        ),
        (
            "battery_present_voltage".into(),
            Value::Num(b.battery_present_voltage as f64),
        ),
    ])
}

fn bix_to_value(b: BixFixedStrings) -> Value {
    // Only the u32 fields are exposed; enum/array fields
    // (`power_unit`, `battery_technology`, `model_number`, ...) have no
    // useful numeric mapping for the host DSL.
    Value::Struct(vec![
        ("revision".into(), Value::Num(b.revision as f64)),
        ("design_capacity".into(), Value::Num(b.design_capacity as f64)),
        (
            "last_full_charge_capacity".into(),
            Value::Num(b.last_full_charge_capacity as f64),
        ),
        ("design_voltage".into(), Value::Num(b.design_voltage as f64)),
        (
            "design_cap_of_warning".into(),
            Value::Num(b.design_cap_of_warning as f64),
        ),
        ("design_cap_of_low".into(), Value::Num(b.design_cap_of_low as f64)),
        ("cycle_count".into(), Value::Num(b.cycle_count as f64)),
        ("measurement_accuracy".into(), Value::Num(b.measurement_accuracy as f64)),
        ("max_sampling_time".into(), Value::Num(b.max_sampling_time as f64)),
        ("min_sampling_time".into(), Value::Num(b.min_sampling_time as f64)),
        (
            "max_averaging_interval".into(),
            Value::Num(b.max_averaging_interval as f64),
        ),
        (
            "min_averaging_interval".into(),
            Value::Num(b.min_averaging_interval as f64),
        ),
        (
            "battery_capacity_granularity_1".into(),
            Value::Num(b.battery_capacity_granularity_1 as f64),
        ),
        (
            "battery_capacity_granularity_2".into(),
            Value::Num(b.battery_capacity_granularity_2 as f64),
        ),
    ])
}

fn caps_to_value(c: TimeAlarmDeviceCapabilities) -> Value {
    Value::Struct(vec![
        ("ac_wake_implemented".into(), Value::Bool(c.ac_wake_implemented())),
        ("dc_wake_implemented".into(), Value::Bool(c.dc_wake_implemented())),
        ("realtime_implemented".into(), Value::Bool(c.realtime_implemented())),
        (
            "realtime_accuracy_in_milliseconds".into(),
            Value::Bool(c.realtime_accuracy_in_milliseconds()),
        ),
        (
            "get_wake_status_supported".into(),
            Value::Bool(c.get_wake_status_supported()),
        ),
        ("ac_s4_wake_supported".into(), Value::Bool(c.ac_s4_wake_supported())),
        ("ac_s5_wake_supported".into(), Value::Bool(c.ac_s5_wake_supported())),
        ("dc_s4_wake_supported".into(), Value::Bool(c.dc_s4_wake_supported())),
        ("dc_s5_wake_supported".into(), Value::Bool(c.dc_s5_wake_supported())),
    ])
}

fn wake_status_to_value(s: TimerStatus) -> Value {
    Value::Struct(vec![
        ("timer_expired".into(), Value::Bool(s.timer_expired())),
        ("timer_triggered_wake".into(), Value::Bool(s.timer_triggered_wake())),
    ])
}

fn timestamp_to_value(t: AcpiTimestamp) -> Value {
    let unix = t.datetime.to_unix_time_seconds() as f64;
    Value::Struct(vec![
        (
            "datetime".into(),
            Value::Struct(vec![
                ("unix_timestamp".into(), Value::Num(unix)),
                ("to_unix_time_seconds".into(), Value::Num(unix)),
            ]),
        ),
        // Convenience top-level alias.
        ("unix_timestamp".into(), Value::Num(unix)),
    ])
}

fn policy_to_value(p: AlarmExpiredWakePolicy) -> Value {
    // AlarmExpiredWakePolicy / AlarmTimerSeconds are `pub struct(pub T)`;
    // expose `.0` and `.value` aliases so scripts can write either.
    let n = p.0 as f64;
    Value::Struct(vec![("0".into(), Value::Num(n)), ("value".into(), Value::Num(n))])
}

fn timer_value_to_value(t: AlarmTimerSeconds) -> Value {
    let n = t.0 as f64;
    Value::Struct(vec![("0".into(), Value::Num(n)), ("value".into(), Value::Num(n))])
}

// ── Verb application ────────────────────────────────────────────────────────

fn apply_verb(verb: &Verb, actual: &Result<Value, String>, env: &Env) -> (bool, String) {
    let resolve = |op: &Operand| -> Result<Value, String> {
        match op {
            Operand::Num(n) => Ok(Value::Num(*n)),
            Operand::Bool(b) => Ok(Value::Bool(*b)),
            Operand::Var(name) => env
                .get(name)
                .cloned()
                .ok_or_else(|| format!("undefined variable `{name}`")),
        }
    };

    let fail = |msg: String| (false, msg);
    let pass = || (true, String::new());

    // is_ok / is_err short-circuit on the Result itself; Unit (setter
    // success) is treated as Ok.
    match verb {
        Verb::IsOk => {
            let ok = verbs::is_ok(actual);
            if ok {
                pass()
            } else {
                fail(format!("expected Ok, got Err({})", actual.as_ref().err().unwrap()))
            }
        }
        Verb::IsErr => {
            let ok = verbs::is_err(actual);
            if ok {
                pass()
            } else {
                fail(format!("expected Err, got Ok({})", actual.as_ref().unwrap()))
            }
        }
        _ => match actual {
            Err(e) => {
                if matches!(
                    verb,
                    Verb::OkEq(_)
                        | Verb::OkGt(_)
                        | Verb::OkGe(_)
                        | Verb::OkLt(_)
                        | Verb::OkLe(_)
                        | Verb::OkInRange { .. }
                ) {
                    fail(format!("expected Ok(_), got Err({e})"))
                } else {
                    fail(format!("call failed: {e}"))
                }
            }
            Ok(value) => apply_value_verb(verb, value, &resolve),
        },
    }
}

fn apply_value_verb(verb: &Verb, value: &Value, resolve: &dyn Fn(&Operand) -> Result<Value, String>) -> (bool, String) {
    let cmp = |op: &Operand, f: &dyn Fn(f64, f64) -> bool, sym: &str| -> (bool, String) {
        match resolve(op) {
            Err(e) => (false, e),
            Ok(rhs) => {
                // Bool equality is supported for eq/ne only; others require numeric.
                match (value, &rhs) {
                    (Value::Bool(a), Value::Bool(b)) if sym == "==" || sym == "!=" => {
                        let ok = if sym == "==" { a == b } else { a != b };
                        if ok {
                            (true, String::new())
                        } else {
                            (false, format!("actual={a} expected{sym}{b}"))
                        }
                    }
                    _ => match (value.as_num(), rhs.as_num()) {
                        (Ok(a), Ok(b)) => {
                            let ok = f(a, b);
                            if ok {
                                (true, String::new())
                            } else {
                                (false, format!("actual={a} expected{sym}{b}"))
                            }
                        }
                        (Err(e), _) | (_, Err(e)) => (false, e),
                    },
                }
            }
        }
    };

    match verb {
        Verb::Eq(op) => cmp(op, &|a, b| verbs::eq(&a, &b), "=="),
        Verb::Ne(op) => cmp(op, &|a, b| verbs::ne(&a, &b), "!="),
        Verb::Gt(op) => cmp(op, &|a, b| verbs::gt(&a, &b), ">"),
        Verb::Ge(op) => cmp(op, &|a, b| verbs::ge(&a, &b), ">="),
        Verb::Lt(op) => cmp(op, &|a, b| verbs::lt(&a, &b), "<"),
        Verb::Le(op) => cmp(op, &|a, b| verbs::le(&a, &b), "<="),
        Verb::InRange { lo, hi, inclusive } => match value.as_num() {
            Err(e) => (false, e),
            Ok(a) => {
                let ok = if *inclusive {
                    verbs::in_range(&a, &(*lo..=*hi))
                } else {
                    verbs::in_range(&a, &(*lo..*hi))
                };
                if ok {
                    (true, String::new())
                } else {
                    (
                        false,
                        format!("actual={a} not in {lo}{}{hi}", if *inclusive { "..=" } else { ".." }),
                    )
                }
            }
        },
        // The `ok_*` family is just a niceness on top of `Result`; here
        // we've already unwrapped Ok(_), so they behave identically to
        // the bare verb.
        Verb::OkEq(op) => cmp(op, &|a, b| verbs::eq(&a, &b), "=="),
        Verb::OkGt(op) => cmp(op, &|a, b| verbs::gt(&a, &b), ">"),
        Verb::OkGe(op) => cmp(op, &|a, b| verbs::ge(&a, &b), ">="),
        Verb::OkLt(op) => cmp(op, &|a, b| verbs::lt(&a, &b), "<"),
        Verb::OkLe(op) => cmp(op, &|a, b| verbs::le(&a, &b), "<="),
        Verb::OkInRange { lo, hi, inclusive } => match value.as_num() {
            Err(e) => (false, e),
            Ok(a) => {
                let ok = if *inclusive {
                    verbs::in_range(&a, &(*lo..=*hi))
                } else {
                    verbs::in_range(&a, &(*lo..*hi))
                };
                if ok {
                    (true, String::new())
                } else {
                    (
                        false,
                        format!("actual={a} not in {lo}{}{hi}", if *inclusive { "..=" } else { ".." }),
                    )
                }
            }
        },
        Verb::IsOk | Verb::IsErr => unreachable!(),
    }
}
