# ec-test-script

Human-readable, text-based integration tests for the [`ec-test-lib`]
EC data sources. Tests are written in a small line-oriented DSL whose
pass/fail vocabulary mirrors the on-target self-test that runs on the
microcontroller, so the same passing condition reads the same way in
firmware and on the host.

```text
let cached = thermal.get_temperature;
sleep 200ms;
thermal.get_temperature                   => ne cached;
battery.get_bst.battery_present_voltage   => in_range 1000..=30000;
rtc.set_timer_value(ac, 300)              => is_ok;
rtc.get_timer_value(ac).value             => le 300;
rtc.get_capabilities.realtime_implemented => eq true;
```

## Running tests

The runner is invoked through `ec-test-cli`:

```pwsh
# In-process mock — no hardware needed.
ec-test-cli --source mock script run path/to/file.test

# Real hardware over serial.
ec-test-cli --source serial --port COM5 script run path/to/file.test

# Windows ACPI (when present).
ec-test-cli --source local script run path/to/file.test

# Run every `.test` file under a directory (recursive, sorted).
ec-test-cli --source mock script run path/to/dir/
```

Each row prints `PASS L<line>: <row>` or `FAIL L<line>: <row> -- <detail>`
to stdout, followed by a per-file `SUMMARY` line. In directory mode a
final `TOTAL` line aggregates across files and the failing files are
listed. The process exits non-zero if any row fails.

You can also drive the runner from Rust:

```rust
use ec_test_lib::mock::Mock;
use ec_test_script::run_script;

let source = Mock::default();
let summary = run_script(&source, include_str!("smoke.test"))?;
println!("{} passed, {} failed", summary.passed, summary.failed);
```

## File format

Test files are UTF-8 text. Whitespace and blank lines are ignored.
Anything from `#` to end of line is a comment. **Every statement must
end with `;`**, and a statement may span multiple physical lines.

```text
# This is a comment.
thermal.get_temperature         # line continues...
    => ok_in_range 0.0..=125.0; # ...statement ends here.
```

## Statements

There are exactly three statement forms.

### 1. Check

```text
<call> => <verb> [<operand> | <range>];
```

Evaluates `<call>`, applies the verb to its result, and records pass
or fail. Most rows are checks.

### 2. Let

```text
let <name> = <call>;
```

Evaluates `<call>` and binds the value to `<name>` for later use as a
verb operand. The row always records as PASS unless the call itself
fails (in which case it records FAIL and the variable is left
unbound).

```text
let cap0 = battery.get_bst.battery_remaining_capacity;
sleep 100ms;
battery.get_bst.battery_remaining_capacity => le cap0;   # didn't grow
```

### 3. Sleep

```text
sleep <duration>;
```

Pauses the runner. Units: `us`, `ms` (default), `s`. The integer must
parse as a `u64`.

```text
sleep 250ms;
sleep 2s;
sleep 500us;
```

## Calls

A call is one of:

```text
<target>.<method>
<target>.<method>(<arg>)
<target>.<method>(<arg>).<accessor>[.<accessor>...]
```

* **`<target>`** is `thermal`, `battery`, or `rtc`.
* **`<method>`** is one of the methods listed below.
* **`<arg>`** is method-specific (enum keyword, numeric literal, a
  comma-separated pair, an identifier previously bound by `let`, or
  the RTC timestamp Buffer literal described below).
  Scalar setter arguments accept any of: number, `true` / `false`, or a
  variable name — so you can snapshot, write, and restore:
  `let orig = thermal.get_rpm; thermal.set_rpm(orig) => is_ok;`.
  Parens are required *only* when the method takes an argument.
* **`.<accessor>`** zero or more dotted field/bitfield projections
  applied to the call's return value. Accessors are identifiers
  (`battery_present_voltage`) or numeric tuple-struct indices (`0`,
  `1`). Trailing `()` on bitfield accessors is allowed and ignored:
  `cap.ac_wake_implemented` and `cap.ac_wake_implemented()` are
  identical.

Projection failures (no such field, wrong type) are reported as a row
failure with a clear `no field 'foo' (have: ...)` message — not a
parse error.

## Targets and methods

### `thermal` — [`ThermalSource`]

| Method                                  | Returns          | Notes                                |
|-----------------------------------------|------------------|--------------------------------------|
| `thermal.get_temperature`               | `Num` (°C)       |                                      |
| `thermal.get_rpm`                       | `Num`            |                                      |
| `thermal.get_min_rpm`                   | `Num`            |                                      |
| `thermal.get_max_rpm`                   | `Num`            |                                      |
| `thermal.get_threshold(<kind>)`         | `Num` (°C)       | `<kind>` ∈ `on`, `ramping`, `max`    |
| `thermal.set_rpm(<rpm>)`                | `Unit`           |                                      |
| `thermal.set_threshold(<kind>, <c>)`    | `Unit`           | e.g. `set_threshold(on, 30.0)`       |

### `battery` — [`BatterySource`]

| Method                | Returns                                   |
|-----------------------|-------------------------------------------|
| `battery.get_bst`     | `Struct` (BST fields, see below)          |
| `battery.get_bix`     | `Struct` (BIX fields, see below)          |
| `battery.set_btp(<n>)`| `Unit`                                    |

**BST fields** (project with `.`):
`battery_state` (numeric raw bitmask),
`battery_present_rate`, `battery_remaining_capacity`,
`battery_present_voltage`.

**BIX fields:** `revision`, `design_capacity`,
`last_full_charge_capacity`, `design_voltage`,
`design_cap_of_warning`, `design_cap_of_low`, `cycle_count`,
`measurement_accuracy`, `max_sampling_time`, `min_sampling_time`,
`max_averaging_interval`, `min_averaging_interval`,
`battery_capacity_granularity_1`, `battery_capacity_granularity_2`.

Enum and byte-array fields (`power_unit`,
`battery_technology`, `model_number`, `serial_number`, …) are
intentionally not exposed — there is no useful numeric mapping for
the verb set.

### `rtc` — [`RtcSource`]

| Method                                                 | Returns      |
|--------------------------------------------------------|--------------|
| `rtc.get_capabilities`                                 | `Struct`     |
| `rtc.get_real_time`                                    | `Struct`     |
| `rtc.set_real_time(Buffer(16) { <bytes> })`             | `Unit`       |
| `rtc.get_wake_status(<id>)`                            | `Struct`     |
| `rtc.get_expired_timer_wake_policy(<id>)`              | `Struct`     |
| `rtc.get_timer_value(<id>)`                            | `Struct`     |
| `rtc.set_timer_value(<id>, <secs>)`                    | `Unit`       |
| `rtc.set_expired_timer_wake_policy(<id>, <u32>)`       | `Unit`       |
| `rtc.clear_wake_status(<id>)`                          | `Unit`       |

`<id>` ∈ `ac`, `dc`.

`set_real_time` takes one ASL-like Buffer literal in the ACPI 16-byte
timestamp layout (multibyte fields are little-endian):

```text
# Sets the test clock to 2026-09-17 12:00:00, UTC, whole seconds.
rtc.set_real_time(Buffer(16) {
    0xEA, 0x07, 9, 17, 12, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
}) => is_ok;
```

The size must be `16` or its hexadecimal equivalent `0x10`. Exactly
16 explicit decimal/hexadecimal byte literals (`0..=255`) are required;
one trailing comma is optional. This is not an ASL evaluator: expressions,
variables, implicit zero padding, Unix timestamps, and field-list arguments
are not supported. Malformed literals or timestamps are parse errors.

The parser checks milliseconds `0..=999` before calling the existing
`AcpiTimestamp::try_from_bytes` decoder, avoiding overflow in its nanosecond
conversion. This range and daylight `0`, `1`, or `3` are compatibility
choices, not normative specification guarantees. ACPI 6.6 lists milliseconds
as `1..=1000`; no correcting erratum has been verified. Linux uses `0` for
whole seconds, as do these examples. No +/-1 conversion is performed.
Byte 7 retains decoder compatibility, without enforcing zero or normalizing
the input; examples use conventional zero. The source receives a typed
`AcpiTimestamp`, whose existing `as_bytes` serializer may emit `1` at byte 7.

**`get_capabilities` accessors** (all `Bool`):
`ac_wake_implemented`, `dc_wake_implemented`, `realtime_implemented`,
`realtime_accuracy_in_milliseconds`, `get_wake_status_supported`,
`ac_s4_wake_supported`, `ac_s5_wake_supported`,
`dc_s4_wake_supported`, `dc_s5_wake_supported`.

**`get_real_time` accessors:** `unix_timestamp` (top-level alias for
`datetime.unix_timestamp`) and the nested
`datetime.{unix_timestamp, to_unix_time_seconds}`.

**`get_wake_status` accessors** (`Bool`): `timer_expired`,
`timer_triggered_wake`.

**`get_timer_value` / `get_expired_timer_wake_policy` accessors**
(`Num`): `.0` (tuple-struct index) or `.value` (alias). Both refer to
the underlying `u32`.

## Verbs

A verb is the word after `=>`. Its operand may be a number literal, a
bool literal (`true` / `false`), a range, or an identifier previously
bound by `let`.

### Numeric and bool verbs

| Verb                          | Passes when                | Operand types |
|-------------------------------|----------------------------|---------------|
| `eq <x>`                      | `actual == x`              | `Num`, `Bool` |
| `ne <x>`                      | `actual != x`              | `Num`, `Bool` |
| `gt <x>`                      | `actual > x`               | `Num`         |
| `ge <x>`                      | `actual >= x`              | `Num`         |
| `lt <x>`                      | `actual < x`               | `Num`         |
| `le <x>`                      | `actual <= x`              | `Num`         |
| `in_range <lo>..<hi>` / `..=` | `actual` in range          | `Num`         |

### Result verbs

`is_ok` / `is_err` operate on the **call result itself**, before any
projection runs. They never inspect the value — useful when you want
to check that a call succeeds without caring what it returns.

| Verb     | Passes when                  |
|----------|------------------------------|
| `is_ok`  | the call returned `Ok(_)`    |
| `is_err` | the call returned `Err(_)`   |

### `ok_*` family

The `ok_*` verbs are exactly equivalent to the bare numeric verbs.
They exist so test files read consistently with their on-target
counterparts (where `ok_eq` reflects that the call returns a
`Result`). Pick whichever reads better — both forms accept variables
and ranges.

```text
thermal.get_max_rpm => ok_eq 6000;     # same as
thermal.get_max_rpm => eq    6000;
```

### Operands

```text
=> eq 42                  # number
=> eq true                # bool
=> eq cached              # identifier, previously bound by `let`
=> in_range 0..100        # half-open
=> in_range 20.0..=40.0   # inclusive
```

Numbers are parsed as `f64`. Identifiers must be ASCII
`[A-Za-z_][A-Za-z0-9_]*`. Variables are looked up at evaluation time;
referencing an unbound name produces a row failure with `undefined
variable 'name'`.

### Verb / value type matching

| Actual value kind                  | Verbs that work                              |
|------------------------------------|----------------------------------------------|
| `Num`                              | every numeric verb, `eq`/`ne`                |
| `Bool`                             | `eq`, `ne` only                              |
| `Unit` (setter success)            | `is_ok` only                                 |
| `Struct` (un-projected)            | none — must project to a leaf first          |

Type mismatches (e.g. `gt true`, `eq 5` against a bool) report a row
failure with a clear message; they do not abort the run.

## Recipes

### Cached-vs-fresh

The pattern from the on-target sampling test:

```text
let cached = thermal.get_temperature;
sleep 200ms;
thermal.get_temperature => ne cached;   # fails if sampling stuck
```

### Round-trip a setter

Snapshot first, write a known value, restore. Note that thermal
thresholds round-trip through deciKelvin and read back within ~0.05 °C
of the written value — use a small tolerance window for the literal
readback, but exact equality holds against a captured value because
it went through the same rounding path.

```text
let original = thermal.get_threshold(on);
thermal.set_threshold(on, 30.0)     => is_ok;
thermal.get_threshold(on)           => ok_in_range 29.9..30.1;
thermal.set_threshold(on, original) => is_ok;
thermal.get_threshold(on)           => eq original;
```

### Cross-call ordering invariant

```text
let on_th = thermal.get_threshold(on);
thermal.get_threshold(ramping) => gt on_th;
thermal.get_threshold(max)     => gt on_th;
```

### Independence of two timers

Real-hardware countdown timers may drift downward between write and
read, so allow a small window rather than asserting exact equality.

```text
rtc.set_timer_value(ac, 600)  => is_ok;
rtc.set_timer_value(dc, 300)  => is_ok;
rtc.get_timer_value(ac).value => le 600;
rtc.get_timer_value(ac).value => gt 590;
rtc.get_timer_value(dc).value => le 300;
rtc.get_timer_value(dc).value => gt 290;
```

### Bitfield checks

```text
rtc.get_capabilities.ac_wake_implemented   => eq true;
rtc.get_capabilities.dc_s5_wake_supported  => eq false;
rtc.get_capabilities.ac_wake_implemented() => eq true;   # parens optional
```

### Clock doesn't go backwards

```text
let t0 = rtc.get_real_time.unix_timestamp;
sleep 1500ms;
rtc.get_real_time.unix_timestamp => gt t0;
```

## Execution semantics

* Statements run **strictly in order**, top to bottom. A failed row
  does not abort the run — the next statement still executes. Use
  this to keep collecting failures in a single pass.
* `let` failures leave the name unbound. Subsequent `eq cached` rows
  then fail with `undefined variable 'cached'`. There is no
  short-circuit "skip the rest of the file" behaviour.
* `sleep` uses `std::thread::sleep` (wall-clock duration).
* Each `<call>` invokes the underlying `Source` method exactly once
  per row. There is no caching between rows; a row written as
  `battery.get_bst.battery_present_voltage` makes a fresh BST call.
  Use `let` to call once and project multiple times against the
  cached value.

## Error reference

Parse errors abort before any row executes and look like:

```text
line 7: expected `=>` separator in `thermal.get_temperature ok 5`
```

Common runtime failure messages (one per failed row):

| Message                            | Meaning                                          |
|------------------------------------|--------------------------------------------------|
| `expected Ok, got Err(...)`        | the call itself returned `Err`                   |
| `no field 'foo' (have: a, b)`      | bad projection accessor                          |
| `expected number, got bool`        | numeric verb on a `Bool` value                   |
| `expected bool, got number`        | `eq true` against a numeric value                |
| `undefined variable 'name'`        | `let name = ...` row failed, or typo             |
| `actual=<x> expected==<y>`         | bare numeric/bool mismatch                       |
| `actual=<x> not in <lo>..<hi>`     | range verb mismatch                              |

## Limitations

* `set_real_time` accepts only the explicit Buffer literal above, not a
  timestamp variable or arbitrary ASL.
* `BatterySource` has no `DeviceId` parameter, so the on-target
  "unknown battery id returns `Err`" rows have no host equivalent.
* No `if` / loops / arithmetic — by design. Use Rust integration
  tests if you need control flow.
* Method-argument strings (and any other quoted literals) are not
  supported; methods that take strings would require a grammar
  extension.

## Platform quirks observed in real hardware

The bundled `examples/` were tuned against a real EC. Things to watch
for when adapting them:

* **Threshold rounding.** `set_threshold(on, 30.0)` reads back as ~30.05
  because the driver stores deciKelvin. Use `ok_in_range` with a tiny
  window for literal readbacks; exact `eq` works against a captured
  value that went through the same rounding.
* **Timer value 0 is "disabled".** `set_timer_value(<id>, 0)` is
  accepted but `get_timer_value(<id>)` reports `u32::MAX` afterward.
  Verify the write succeeds; don't assert the read-back equals 0.
* **Timer drift.** Countdown timers may decrement between write and
  read; use a `le N` + `gt N-10` pair instead of exact equality.
* **Capability bits vary by platform.** Don't hardcode individual
  bits; the bundled scripts only assert `realtime_implemented` (which
  must be true if the RTC just answered).
* **BIX capacity fields may be stubbed.** On some platforms
  `design_capacity`, `last_full_charge_capacity`, `design_cap_of_warning`,
  and `design_cap_of_low` return `0` or `0xDEADBEEF`. The bundled
  `battery.test` skips these.

## See also

* [`ec-test-lib`] — the `Source` traits the runner targets.
* `examples/` — `thermal`, `battery`, `rtc`, `advanced`, and `full`
  reference scripts.
* `tests/mock.rs` — parse-and-execute smoke tests against the
  in-process `Mock` source. They guard against parser regressions and
  dispatcher panics; row failures are tolerated because the scripts
  assert real-hardware behaviour the Mock doesn't fully simulate.

[`ec-test-lib`]: ../test-lib/
[`ThermalSource`]: ../test-lib/src/lib.rs
[`BatterySource`]: ../test-lib/src/lib.rs
[`RtcSource`]: ../test-lib/src/lib.rs
