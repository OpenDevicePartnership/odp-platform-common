# PR review instructions
CI will build, run cargo test, run cargo clippy, feature combinations are checked with cargo hack, do not comment on compile errors/warnings, nor clippy warnings.

Pay special attention to...
* code that uses `select`, `selectN`, `select_array`, `select_slice`, or is marked with a drop safety comment. `select` functions drop the other futures. Check that values are not lost when this happens.
* code that could possibly panic or is marked with a panic safety comment.