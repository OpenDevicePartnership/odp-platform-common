# Matched-Policy Boot Manager Benchmark Contract

## Purpose

The performance program needs two distinct comparisons:

1. **Product-path comparison**: stock C `BdsDxe` versus the proposed Rust
   product integration. This answers whether an OEM observes a material boot
   regression after adopting the new architecture.
2. **Implementation comparison**: benchmark-only C and Rust BDS providers that
   execute the same observable policy. This bounds the cost attributable to
   the implementation rather than product-policy differences.

The clean three-run result, 4.306 seconds for `patina_boot` versus 4.320 seconds
for stock C BDS, belongs to the first lane. It supports whole-firmware parity,
not an intrinsic implementation-cost claim.

## Required invariants

Every accepted sample must satisfy all of the following:

- same physical device, power mode, OS image, and firmware configuration;
- same Patina DXE core, platform drivers, compiler profile, and logging level;
- normal boot mode `0x2`;
- no staged capsule and no flash-update boot behavior;
- identical Secure Boot state;
- identical `BootOrder` and active `Boot####` variables;
- no firmware setup, hotkey, recovery, or interactive UI entry;
- valid FPDT `ResetEnd`, OS-loader load/start, and ExitBootServices records;
- image hashes and build provenance captured with the sample.

A sample that fails any invariant is discarded before timing analysis.

## Minimal observable policy

The benchmark-only C and Rust providers must execute this sequence:

1. Report Microsoft Start of BDS.
2. Complete remaining DXE dispatch.
3. Connect the same controller set with the same recursion and retry limits.
4. Signal the MU capsule-processing event.
5. Signal EndOfDxe.
6. Install DxeSmmReadyToLock.
7. Read `BootOrder` and active `Boot####` options in listed order.
8. Signal ReadyToBoot immediately before each boot attempt.
9. Report the standard OS-loader LoadImage progress code.
10. Call boot-policy `LoadImage`.
11. Arm the five-minute watchdog.
12. Report the standard OS-loader StartImage progress code.
13. Call `StartImage`.
14. Clear the watchdog if the image returns.
15. Continue to the next option after a returned or failed image.

The benchmark policy excludes:

- `BootNext` and `BootCurrent`;
- console discovery or connection;
- Driver, SysPrep, PlatformRecovery, and Key options;
- firmware setup and boot-manager UI;
- OEM recovery and provisioning behavior;
- synthesized filesystem fallback;
- diagnostics, memory testing, and platform-specific pre/post hooks.

These exclusions make the pair suitable for implementation attribution. They
also mean the minimal C provider is not a replacement product baseline.

## Equivalence checklist

The pair is ready to measure only when source review and traces confirm:

| Contract point | C provider | Rust provider |
| --- | --- | --- |
| Start-of-BDS event | Required | Required |
| DXE dispatch completion | Same termination rule | Same termination rule |
| Controller selection | Same handles and USB policy | Same handles and USB policy |
| Capsule event | Same position | Same position |
| EndOfDxe | Fail closed | Fail closed |
| DxeSmmReadyToLock | Same position | Same position |
| Boot option parsing | Active BootOrder entries only | Active BootOrder entries only |
| ReadyToBoot | Before every attempt | Before every attempt |
| LoadImage boot policy | `TRUE` | `true` |
| Watchdog | 300 seconds | 300 seconds |
| Returned image | Clear watchdog, try next | Clear watchdog, try next |
| Failure terminal state | Same terminal behavior | Same terminal behavior |

No performance result is accepted while a row differs.

## Shared phase records

Both providers must emit records at the same semantic boundaries:

- `BmmDxeDispatchStart`
- `BmmDxeDispatchEnd`
- `BmmConnectStart`
- `BmmConnectEnd`
- `BmmCapsuleEventStart`
- `BmmCapsuleEventEnd`
- `BmmEndOfDxeStart`
- `BmmEndOfDxeEnd`
- `BmmReadyToLockEnd`
- `BmmBootDiscoveryStart`
- `BmmBootDiscoveryEnd`
- standard OS-loader LoadImage progress
- standard OS-loader StartImage progress
- standard ExitBootServices entry and exit

The custom phase transport must use one shared definition consumed by the C
and Rust implementations. Marker names and positions are fixed by this
contract; the implementation may use status-code data or the platform
performance protocol after confirming that both paths produce the same
timestamp source and extraction format.

## Measurement protocol

- collect at least 10 accepted warm boots per arm;
- randomize and interleave arm order rather than running one arm entirely
  before the other;
- include an agreed cold-boot sample as a separate dataset;
- report mean, median, standard deviation, p50, p95, range, and confidence
  interval;
- report paired whole-boot and phase deltas;
- retain raw FPDT, phase records, image hashes, firmware version, boot
  variables, and sample rejection reasons.

The analysis must distinguish:

- shared PEI and DXE time;
- remaining DXE dispatch;
- controller connection;
- security-transition events;
- boot-option discovery;
- `LoadImage`;
- OS loader execution before ExitBootServices;
- ExitBootServices.

## Decision outputs

The product-path comparison answers:

> Does adopting the Rust boot architecture produce a material end-user boot
> regression on the target platform?

The matched-policy comparison answers:

> With observable policy held constant, what bound can be placed on the C
> versus Rust boot-manager implementation cost, and which phase explains it?

Neither lane alone proves architectural quality or security. Those claims use
the semantic-parity, fault-injection, fuzzing, and Secure Boot evidence in the
Boot Manager Modernization roadmap.
