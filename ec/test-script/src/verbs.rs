//! Pure verb-evaluation primitives. Inlined so the runner has no
//! cross-repo dependency on the firmware `embedded-service-test`
//! crate; the on-target macros define the same semantics inline.
//!
//! SPDX-License-Identifier: MIT

use std::ops::RangeBounds;

#[inline]
pub fn eq<T: PartialEq>(a: &T, b: &T) -> bool {
    a == b
}

#[inline]
pub fn ne<T: PartialEq>(a: &T, b: &T) -> bool {
    a != b
}

#[inline]
pub fn gt<T: PartialOrd>(a: &T, b: &T) -> bool {
    a > b
}

#[inline]
pub fn ge<T: PartialOrd>(a: &T, b: &T) -> bool {
    a >= b
}

#[inline]
pub fn lt<T: PartialOrd>(a: &T, b: &T) -> bool {
    a < b
}

#[inline]
pub fn le<T: PartialOrd>(a: &T, b: &T) -> bool {
    a <= b
}

#[inline]
pub fn in_range<T: PartialOrd, R: RangeBounds<T>>(a: &T, range: &R) -> bool {
    range.contains(a)
}

#[inline]
pub fn is_ok<T, E>(r: &Result<T, E>) -> bool {
    r.is_ok()
}

#[inline]
pub fn is_err<T, E>(r: &Result<T, E>) -> bool {
    r.is_err()
}
