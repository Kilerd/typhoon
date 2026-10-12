//! `s[a:b]`: slicing a `str`.
//!
//! The offsets are **code points**, the unit `len(s)` and `s.find(sub)` already
//! use, so `s[s.find(x):]` means what it says. Python's rules apply: a negative
//! offset counts from the end, an out-of-range one is clamped rather than an
//! error, an omitted bound is the start or the end, and an empty range is the
//! empty string. Because every code point offset lies on a UTF-8 boundary, a
//! slice can never cut a character in half.
//!
//! The cost is O(b - a) for ASCII text — `char_len == byte_len` means the code
//! point offsets *are* byte offsets — and one O(n) scan otherwise. A one-byte
//! ASCII result comes from the static table `for c in s` uses, and the whole
//! string is returned as itself, so neither allocates.

use crate::{ascii_str_value, str_bytes, str_char_len, str_new_with_char_len};

/// Python's clamping of one slice bound against a length of `n`.
///
/// ```
/// # use typhoon_runtime::slice::clamp_bound;
/// assert_eq!(clamp_bound(-2, 5), 3);
/// assert_eq!(clamp_bound(-9, 5), 0);
/// assert_eq!(clamp_bound(7, 5), 5);
/// ```
pub fn clamp_bound(index: i64, n: i64) -> i64 {
    if index < 0 {
        index.saturating_add(n).max(0)
    } else {
        index.min(n)
    }
}

/// The byte offsets of the code points `a` and `b` (`a <= b`) of `bytes`.
pub fn char_range_to_bytes(bytes: &[u8], a: i64, b: i64) -> (usize, usize) {
    let mut start = bytes.len();
    let mut end = bytes.len();
    let mut chars = 0i64;
    for (at, byte) in bytes.iter().enumerate() {
        if byte & 0xC0 == 0x80 {
            continue;
        }
        if chars == a {
            start = at;
        }
        if chars == b {
            end = at;
            break;
        }
        chars += 1;
    }
    (start.min(end), end)
}

/// The empty string, statically: `{ byte_len: 0, char_len: 0 }`.
static EMPTY_STR: [i64; 2] = [0, 0];

/// `s[start:stop]` in code points, with Python's negative offsets and
/// clamping. An omitted start is passed as `0` and an omitted stop as
/// `i64::MAX`.
///
/// # Safety
///
/// `s` must point at a well-formed string header.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ty_str_slice(s: *const u8, start: i64, stop: i64) -> *const u8 {
    unsafe {
        let n = str_char_len(s);
        let a = clamp_bound(start, n);
        let b = clamp_bound(stop, n);
        if a >= b {
            return EMPTY_STR.as_ptr() as *const u8;
        }
        if a == 0 && b == n {
            // `str` is immutable: the whole string is the slice.
            return s;
        }
        let bytes = str_bytes(s);
        if bytes.len() as i64 == n {
            let (a, b) = (a as usize, b as usize);
            if b - a == 1 && bytes[a] < 0x80 {
                return ascii_str_value(bytes[a]);
            }
            return str_new_with_char_len(&bytes[a..b], (b - a) as i64);
        }
        let (lo, hi) = char_range_to_bytes(bytes, a, b);
        str_new_with_char_len(&bytes[lo..hi], b - a)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::str_new;

    fn slice(text: &str, a: i64, b: i64) -> String {
        unsafe {
            let s = str_new(text.as_bytes());
            let out = ty_str_slice(s, a, b);
            let bytes = str_bytes(out);
            assert_eq!(
                str_char_len(out),
                crate::char_count(bytes),
                "char_len of {text:?}[{a}:{b}]"
            );
            String::from_utf8(bytes.to_vec()).unwrap()
        }
    }

    #[test]
    fn ascii_slices_follow_python() {
        assert_eq!(slice("typhoon", 1, 4), "yph");
        assert_eq!(slice("typhoon", 0, i64::MAX), "typhoon");
        assert_eq!(slice("typhoon", -4, i64::MAX), "hoon");
        assert_eq!(slice("typhoon", 0, -1), "typhoo");
        assert_eq!(slice("typhoon", 5, 2), "");
        assert_eq!(slice("typhoon", 100, 200), "");
        assert_eq!(slice("typhoon", -100, 2), "ty");
        assert_eq!(slice("typhoon", 3, 4), "h");
        assert_eq!(slice("", 0, i64::MAX), "");
        assert_eq!(slice("x", i64::MIN, i64::MAX), "x");
    }

    #[test]
    fn non_ascii_slices_count_code_points() {
        assert_eq!(slice("台风typhoon", 0, 2), "台风");
        assert_eq!(slice("台风typhoon", 1, 3), "风t");
        assert_eq!(slice("héllo", -3, i64::MAX), "llo");
        assert_eq!(slice("héllo", 1, 2), "é");
        assert_eq!(slice("a😀b", 1, 2), "😀");
        assert_eq!(slice("a😀b", 2, 3), "b");
    }
}
