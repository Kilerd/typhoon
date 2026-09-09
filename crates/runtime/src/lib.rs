//! `typhoon-runtime` — the C-ABI runtime linked into every Typhoon binary.
//!
//! See `docs/DESIGN.md` §5 (memory management) and §6.1 (crate layout).
//!
//! The crate is built as a `staticlib` (`libtyphoon_runtime.a`). Generated LLVM
//! IR declares the `ty_*` symbols below and `clang` links the archive together
//! with Boehm GC (`-lgc`) into the final executable:
//!
//! ```sh
//! clang -O2 out.ll libtyphoon_runtime.a -lgc -o out
//! ```
//!
//! # Calling contract
//!
//! * [`ty_rt_init`] must be called once, first, from `main`.
//! * `print(a, b)` is lowered to `print_a` / [`ty_print_sep`] / `print_b` /
//!   [`ty_print_end`] (DESIGN §4.6: single space separator, trailing newline).
//! * stdout is buffered process-wide; it is flushed by [`ty_rt_exit`],
//!   [`ty_panic`] and by an `atexit` hook installed by [`ty_rt_init`], so a
//!   plain `return 0` from `main` still produces complete output.

use std::ffi::{c_int, c_void};
use std::io::{self, BufWriter, Stdout, Write};
use std::sync::{Mutex, MutexGuard, OnceLock};

// Boehm-Demers-Weiser GC. Declared only: the archive does not link it, the
// final `clang` invocation does (`-lgc`).
unsafe extern "C" {
    fn GC_init();
    fn GC_malloc(n: usize) -> *mut c_void;
    fn GC_malloc_atomic(n: usize) -> *mut c_void;
    fn atexit(cb: extern "C" fn()) -> c_int;
}

/// Exit code used for every runtime panic (matches Rust's own convention).
pub const PANIC_EXIT_CODE: i32 = 101;

// ---------------------------------------------------------------------------
// Buffered stdout
// ---------------------------------------------------------------------------

static OUT: OnceLock<Mutex<BufWriter<Stdout>>> = OnceLock::new();

/// Locks the process-global buffered stdout, recovering from poisoning.
fn out() -> MutexGuard<'static, BufWriter<Stdout>> {
    OUT.get_or_init(|| Mutex::new(BufWriter::with_capacity(64 * 1024, io::stdout())))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Flushes the buffered stdout, ignoring errors (nothing sensible to do).
fn flush_out() {
    let _ = out().flush();
}

extern "C" fn flush_at_exit() {
    flush_out();
}

// ---------------------------------------------------------------------------
// Lifecycle
// ---------------------------------------------------------------------------

/// Initialises the runtime: starts Boehm GC and installs the stdout flush hook.
///
/// Must be the first runtime call made by a Typhoon program.
#[unsafe(no_mangle)]
pub extern "C" fn ty_rt_init() {
    unsafe {
        GC_init();
        atexit(flush_at_exit);
    }
}

/// Flushes stdout and terminates the process with `code`.
#[unsafe(no_mangle)]
pub extern "C" fn ty_rt_exit(code: i32) -> ! {
    flush_out();
    std::process::exit(code)
}

// ---------------------------------------------------------------------------
// Allocation
// ---------------------------------------------------------------------------

/// Allocates `size` GC-traced bytes. The block *is* scanned for pointers.
///
/// Aborts the process with `panic: out of memory` when the GC cannot satisfy
/// the request. Never returns null.
#[unsafe(no_mangle)]
pub extern "C" fn ty_alloc(size: usize) -> *mut u8 {
    let p = unsafe { GC_malloc(size) } as *mut u8;
    if p.is_null() {
        out_of_memory();
    }
    p
}

/// Allocates `size` GC-managed bytes that are known to contain no pointers
/// (string bytes, `list<int>` payloads, …). The block is *not* scanned.
///
/// Aborts the process with `panic: out of memory` when the GC cannot satisfy
/// the request. Never returns null.
#[unsafe(no_mangle)]
pub extern "C" fn ty_alloc_atomic(size: usize) -> *mut u8 {
    let p = unsafe { GC_malloc_atomic(size) } as *mut u8;
    if p.is_null() {
        out_of_memory();
    }
    p
}

fn out_of_memory() -> ! {
    let msg = b"out of memory";
    unsafe { ty_panic(msg.as_ptr(), msg.len()) }
}

// ---------------------------------------------------------------------------
// Panics
// ---------------------------------------------------------------------------

/// Aborts the program with `panic: <msg>` on stderr and exit code 101.
///
/// # Safety
///
/// `msg_ptr` must point to `msg_len` readable bytes (UTF-8 is expected but not
/// required; the bytes are written verbatim).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ty_panic(msg_ptr: *const u8, msg_len: usize) -> ! {
    flush_out();
    let msg = unsafe { bytes(msg_ptr, msg_len) };
    let mut err = io::stderr().lock();
    let _ = write_panic(&mut err, msg);
    let _ = err.flush();
    std::process::exit(PANIC_EXIT_CODE)
}

/// Renders a panic message: `panic: <msg>\n`.
pub fn write_panic(w: &mut impl Write, msg: &[u8]) -> io::Result<()> {
    w.write_all(b"panic: ")?;
    w.write_all(msg)?;
    w.write_all(b"\n")
}

// ---------------------------------------------------------------------------
// print
// ---------------------------------------------------------------------------

/// Writes an `int` in decimal (DESIGN §4.6).
pub fn write_int(w: &mut impl Write, value: i64) -> io::Result<()> {
    let mut buf = itoa(value);
    w.write_all(buf.as_bytes_mut())
}

/// Writes a `float` using Python's `repr(float)` spelling (DESIGN §4.6).
pub fn write_float(w: &mut impl Write, value: f64) -> io::Result<()> {
    w.write_all(format_float(value).as_bytes())
}

/// Writes a `bool` as `True` / `False` (DESIGN §4.6).
pub fn write_bool(w: &mut impl Write, value: u8) -> io::Result<()> {
    w.write_all(if value != 0 {
        b"True" as &[u8]
    } else {
        b"False"
    })
}

/// Writes a `str` verbatim, without quotes (DESIGN §4.6).
pub fn write_str(w: &mut impl Write, value: &[u8]) -> io::Result<()> {
    w.write_all(value)
}

/// Writes `None` (DESIGN §4.6).
pub fn write_none(w: &mut impl Write) -> io::Result<()> {
    w.write_all(b"None")
}

/// Writes the separator between two `print` arguments: a single space.
pub fn write_sep(w: &mut impl Write) -> io::Result<()> {
    w.write_all(b" ")
}

/// Writes the terminator of a `print` call: a newline.
pub fn write_end(w: &mut impl Write) -> io::Result<()> {
    w.write_all(b"\n")
}

/// Prints an `int`.
#[unsafe(no_mangle)]
pub extern "C" fn ty_print_int(value: i64) {
    let _ = write_int(&mut *out(), value);
}

/// Prints a `float` in Python `repr` form.
#[unsafe(no_mangle)]
pub extern "C" fn ty_print_float(value: f64) {
    let _ = write_float(&mut *out(), value);
}

/// Prints a `bool` as `True` / `False`.
#[unsafe(no_mangle)]
pub extern "C" fn ty_print_bool(value: u8) {
    let _ = write_bool(&mut *out(), value);
}

/// Prints a `str` given as pointer + byte length.
///
/// # Safety
///
/// `ptr` must point to `len` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ty_print_str(ptr: *const u8, len: usize) {
    let s = unsafe { bytes(ptr, len) };
    let _ = write_str(&mut *out(), s);
}

/// Prints `None`.
#[unsafe(no_mangle)]
pub extern "C" fn ty_print_none() {
    let _ = write_none(&mut *out());
}

/// Prints the single-space separator between `print` arguments.
#[unsafe(no_mangle)]
pub extern "C" fn ty_print_sep() {
    let _ = write_sep(&mut *out());
}

/// Prints the trailing newline of a `print` call.
#[unsafe(no_mangle)]
pub extern "C" fn ty_print_end() {
    let _ = write_end(&mut *out());
}

/// Reads `len` bytes at `ptr` as a slice, tolerating a null pointer when
/// `len == 0`.
///
/// # Safety
///
/// `ptr` must point to `len` readable bytes (or `len` must be zero).
unsafe fn bytes<'a>(ptr: *const u8, len: usize) -> &'a [u8] {
    if len == 0 {
        return &[];
    }
    debug_assert!(!ptr.is_null());
    unsafe { std::slice::from_raw_parts(ptr, len) }
}

// ---------------------------------------------------------------------------
// Integer formatting
// ---------------------------------------------------------------------------

/// A tiny stack buffer holding the decimal spelling of an `i64`.
struct IntBuf {
    buf: [u8; 20],
    len: usize,
}

impl IntBuf {
    fn as_bytes_mut(&mut self) -> &[u8] {
        &self.buf[..self.len]
    }
}

/// Formats `value` in decimal without allocating.
fn itoa(value: i64) -> IntBuf {
    let mut tmp = [0u8; 20];
    let mut n = 0usize;
    let negative = value < 0;
    // Use the unsigned magnitude so that i64::MIN is handled correctly.
    let mut mag = value.unsigned_abs();
    loop {
        tmp[n] = b'0' + (mag % 10) as u8;
        n += 1;
        mag /= 10;
        if mag == 0 {
            break;
        }
    }
    let mut buf = [0u8; 20];
    let mut len = 0usize;
    if negative {
        buf[0] = b'-';
        len = 1;
    }
    while n > 0 {
        n -= 1;
        buf[len] = tmp[n];
        len += 1;
    }
    IntBuf { buf, len }
}

// ---------------------------------------------------------------------------
// Float formatting (Python `repr(float)`)
// ---------------------------------------------------------------------------

/// Splits a Rust `LowerExp` rendering (`d[.ddd]e<exp>`) into mantissa and
/// decimal exponent.
fn split_exp(repr: &str) -> (&str, i32) {
    let (mantissa, exp) = repr.split_once('e').expect("LowerExp always emits `e`");
    let exp = exp
        .parse()
        .expect("LowerExp always emits an integer exponent");
    (mantissa, exp)
}

/// Formats `value` exactly the way CPython's `repr(float)` does.
///
/// The rules (DESIGN §4.6) are:
///
/// * shortest representation that round-trips through `f64`;
/// * fixed notation while `-4 < decpt <= 16`, where `decpt` is the position of
///   the decimal point (`value = 0.<digits> * 10^decpt`), otherwise exponent
///   notation with a signed, at-least-two-digit exponent (`1e+16`, `1e-05`);
/// * the result always contains a `.` or an `e`, so `5.0` never prints as `5`;
/// * `inf`, `-inf`, `nan`, and a signed `-0.0`.
///
/// ```
/// # use typhoon_runtime::format_float;
/// assert_eq!(format_float(5.0), "5.0");
/// assert_eq!(format_float(1e16), "1e+16");
/// assert_eq!(format_float(1e-5), "1e-05");
/// ```
pub fn format_float(value: f64) -> String {
    if value.is_nan() {
        return "nan".to_string();
    }
    if value.is_infinite() {
        return if value < 0.0 { "-inf" } else { "inf" }.to_string();
    }

    let negative = value.is_sign_negative();
    let sign = if negative { "-" } else { "" };
    if value == 0.0 {
        return format!("{sign}0.0");
    }

    // Rust's `LowerExp` for f64 emits the shortest round-trip digit string as
    // `d[.ddd]e<exp>`. It agrees with CPython's `dtoa` everywhere except on an
    // exact tie, where two equally-short, equally-distant decimals both
    // round-trip: Rust rounds half away from zero, CPython rounds half to even
    // (`repr(-2227059643032627.25)` is `-2227059643032627.2`, not `...3`).
    //
    // A tie can only be resolved differently when Rust's last digit came out
    // odd, so in that case we re-render the same number of significant digits
    // with `{:.*e}`, which *does* round half to even, and use that instead.
    let repr = format!("{:e}", value.abs());
    let (mantissa, exp) = split_exp(&repr);
    let significant = mantissa.len() - usize::from(mantissa.contains('.'));
    let owned;
    let (mantissa, exp) = if mantissa.as_bytes()[mantissa.len() - 1] % 2 == 1 {
        owned = format!("{:.*e}", significant - 1, value.abs());
        split_exp(&owned)
    } else {
        (mantissa, exp)
    };

    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let digits = digits.trim_end_matches('0');
    let digits = if digits.is_empty() { "0" } else { digits };

    // `decpt` counts the digits that belong in front of the decimal point.
    let decpt = exp + 1;

    if decpt <= -4 || decpt > 16 {
        // Exponent notation. CPython does not pad the mantissa with `.0` here.
        let e = decpt - 1;
        let (esign, emag) = if e < 0 { ("-", -e) } else { ("+", e) };
        let mut s = String::with_capacity(digits.len() + 8);
        s.push_str(sign);
        s.push_str(&digits[..1]);
        if digits.len() > 1 {
            s.push('.');
            s.push_str(&digits[1..]);
        }
        s.push('e');
        s.push_str(esign);
        if emag < 10 {
            s.push('0');
        }
        s.push_str(&emag.to_string());
        return s;
    }

    let mut s = String::with_capacity(digits.len() + 20);
    s.push_str(sign);
    if decpt <= 0 {
        s.push_str("0.");
        for _ in 0..-decpt {
            s.push('0');
        }
        s.push_str(digits);
    } else {
        let decpt = decpt as usize;
        if decpt >= digits.len() {
            s.push_str(digits);
            for _ in 0..(decpt - digits.len()) {
                s.push('0');
            }
            s.push_str(".0");
        } else {
            s.push_str(&digits[..decpt]);
            s.push('.');
            s.push_str(&digits[decpt..]);
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rendered(f: impl FnOnce(&mut Vec<u8>) -> io::Result<()>) -> String {
        let mut buf = Vec::new();
        f(&mut buf).unwrap();
        String::from_utf8(buf).unwrap()
    }

    // -- format_float --------------------------------------------------

    #[test]
    fn float_repr_matches_python() {
        let cases: &[(f64, &str)] = &[
            (5.0, "5.0"),
            (3.5, "3.5"),
            (0.1, "0.1"),
            (1.0 / 3.0, "0.3333333333333333"),
            (1e15, "1000000000000000.0"),
            (1e16, "1e+16"),
            (1e-4, "0.0001"),
            (1e-5, "1e-05"),
            (2.5e-10, "2.5e-10"),
            (1.5e300, "1.5e+300"),
            (123456789.123, "123456789.123"),
            (0.0, "0.0"),
            (-0.0, "-0.0"),
            (100.0, "100.0"),
            (1e22, "1e+22"),
        ];
        for (value, expected) in cases {
            assert_eq!(format_float(*value), *expected, "repr({value:?})");
        }
    }

    #[test]
    fn float_repr_sqrt_two() {
        assert_eq!(format_float(2.0f64.powf(0.5)), "1.4142135623730951");
    }

    #[test]
    fn float_repr_non_finite() {
        assert_eq!(format_float(f64::INFINITY), "inf");
        assert_eq!(format_float(f64::NEG_INFINITY), "-inf");
        assert_eq!(format_float(f64::NAN), "nan");
        assert_eq!(format_float(-f64::NAN), "nan");
    }

    #[test]
    fn float_repr_negatives() {
        assert_eq!(format_float(-5.0), "-5.0");
        assert_eq!(format_float(-0.1), "-0.1");
        assert_eq!(format_float(-1e16), "-1e+16");
        assert_eq!(format_float(-1e-5), "-1e-05");
    }

    #[test]
    fn float_repr_boundaries() {
        // The fixed/exponent switch lives at decpt == 16.
        assert_eq!(format_float(9999999999999998.0), "9999999999999998.0");
        assert_eq!(format_float(1e17), "1e+17");
        assert_eq!(format_float(0.001), "0.001");
        assert_eq!(format_float(0.0001), "0.0001");
        assert_eq!(format_float(0.00001), "1e-05");
        assert_eq!(format_float(1.5e-5), "1.5e-05");
    }

    #[test]
    fn float_repr_breaks_exact_ties_to_even() {
        // The exact value is `...27.25`: both `...27.2` and `...27.3` are
        // 17 digits and round-trip. CPython picks the even last digit; Rust's
        // own shortest formatter would pick `...3`.
        assert_eq!(format_float(-2227059643032627.2), "-2227059643032627.2");
        assert_eq!(format_float(2160375749930557.2), "2160375749930557.2");
        assert_eq!(format_float(-246856750993115.62), "-246856750993115.62");
        assert_eq!(format_float(1181537280768007.2), "1181537280768007.2");
    }

    #[test]
    fn float_repr_extremes() {
        assert_eq!(format_float(f64::MIN_POSITIVE), "2.2250738585072014e-308");
        assert_eq!(format_float(5e-324), "5e-324");
        assert_eq!(format_float(f64::MAX), "1.7976931348623157e+308");
    }

    #[test]
    fn float_repr_always_has_dot_or_exponent() {
        let samples = [
            0.0, -0.0, 1.0, -1.0, 42.0, 1e15, 1e16, 1e-4, 1e-5, 2.5e-10, 1e300, 6.02e23,
        ];
        for v in samples {
            let s = format_float(v);
            assert!(s.contains('.') || s.contains('e'), "{v} formatted as {s}");
        }
    }

    #[test]
    fn float_repr_round_trips() {
        let samples = [
            0.1,
            1.0 / 3.0,
            2.0f64.sqrt(),
            std::f64::consts::PI,
            1e-5,
            1e16,
            1.5e300,
            5e-324,
            f64::MAX,
            f64::MIN_POSITIVE,
            -123456789.123,
        ];
        for v in samples {
            let s = format_float(v);
            let back: f64 = s.parse().unwrap();
            assert_eq!(back.to_bits(), v.to_bits(), "{s} did not round-trip");
        }
    }

    // -- write_* -------------------------------------------------------

    #[test]
    fn writes_ints() {
        assert_eq!(rendered(|w| write_int(w, 0)), "0");
        assert_eq!(rendered(|w| write_int(w, 7)), "7");
        assert_eq!(rendered(|w| write_int(w, -42)), "-42");
        assert_eq!(rendered(|w| write_int(w, i64::MAX)), "9223372036854775807");
        assert_eq!(rendered(|w| write_int(w, i64::MIN)), "-9223372036854775808");
    }

    #[test]
    fn writes_floats() {
        assert_eq!(rendered(|w| write_float(w, 5.0)), "5.0");
        assert_eq!(rendered(|w| write_float(w, -0.0)), "-0.0");
        assert_eq!(rendered(|w| write_float(w, 1e16)), "1e+16");
    }

    #[test]
    fn writes_bools() {
        assert_eq!(rendered(|w| write_bool(w, 1)), "True");
        assert_eq!(rendered(|w| write_bool(w, 0)), "False");
        // Any non-zero byte is truthy.
        assert_eq!(rendered(|w| write_bool(w, 255)), "True");
    }

    #[test]
    fn writes_strings_verbatim() {
        assert_eq!(
            rendered(|w| write_str(w, b"Hello, Typhoon!")),
            "Hello, Typhoon!"
        );
        assert_eq!(rendered(|w| write_str(w, b"")), "");
        assert_eq!(rendered(|w| write_str(w, "台风".as_bytes())), "台风");
        // No quoting, no escaping.
        assert_eq!(rendered(|w| write_str(w, b"a\"b\\c")), "a\"b\\c");
    }

    #[test]
    fn writes_none_sep_end() {
        assert_eq!(rendered(write_none), "None");
        assert_eq!(rendered(write_sep), " ");
        assert_eq!(rendered(write_end), "\n");
    }

    #[test]
    fn writes_panic_message() {
        assert_eq!(
            rendered(|w| write_panic(w, b"index out of range")),
            "panic: index out of range\n"
        );
    }

    #[test]
    fn print_lowering_of_two_arguments() {
        // `print(-42, 5.0)` lowers to int / sep / float / end.
        let s = rendered(|w| {
            write_int(w, -42)?;
            write_sep(w)?;
            write_float(w, 5.0)?;
            write_end(w)
        });
        assert_eq!(s, "-42 5.0\n");
    }

    #[test]
    fn print_lowering_of_mixed_arguments() {
        let s = rendered(|w| {
            write_str(w, b"x")?;
            write_sep(w)?;
            write_bool(w, 1)?;
            write_sep(w)?;
            write_none(w)?;
            write_end(w)
        });
        assert_eq!(s, "x True None\n");
    }

    // -- allocation ----------------------------------------------------

    #[test]
    fn gc_allocation_works() {
        ty_rt_init();
        for i in 1..=64usize {
            let size = i * 1024;
            let p = ty_alloc(size);
            assert!(!p.is_null());
            unsafe { std::ptr::write_bytes(p, 0xAB, size) };
            assert_eq!(unsafe { *p }, 0xAB);

            let q = ty_alloc_atomic(size);
            assert!(!q.is_null());
            unsafe { std::ptr::write_bytes(q, 0xCD, size) };
            assert_eq!(unsafe { *q.add(size - 1) }, 0xCD);
        }
    }

    #[test]
    fn zero_length_string_is_safe() {
        let s = unsafe { bytes(std::ptr::null(), 0) };
        assert!(s.is_empty());
    }
}
