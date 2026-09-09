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
use std::sync::{Mutex, MutexGuard, Once, OnceLock};

// Boehm-Demers-Weiser GC. Declared only: the archive does not link it, the
// final `clang` invocation does (`-lgc`).
unsafe extern "C" {
    fn GC_init();
    // Unused in `cfg(test)`, where allocation goes through Rust's allocator;
    // see `raw_alloc`.
    #[cfg_attr(test, allow(dead_code))]
    fn GC_malloc(n: usize) -> *mut c_void;
    #[cfg_attr(test, allow(dead_code))]
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
/// Must be the first runtime call made by a Typhoon program. Calling it more
/// than once, or from several threads at once, is harmless: the work happens
/// exactly once (Boehm's `GC_init` is not itself reentrant).
#[unsafe(no_mangle)]
pub extern "C" fn ty_rt_init() {
    static INIT: Once = Once::new();
    INIT.call_once(|| unsafe {
        GC_init();
        atexit(flush_at_exit);
    });
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

/// The raw block allocator behind [`ty_alloc`] and [`ty_alloc_atomic`].
///
/// In a real Typhoon binary this is Boehm GC. Under `cargo test` it is Rust's
/// allocator instead, because `cargo test` runs every test on its own thread
/// and those threads are not registered with the GC — a Typhoon program is
/// single-threaded, so the runtime never calls `GC_register_my_thread`, and
/// allocating from an unregistered thread makes Boehm collect live objects and
/// abort in `thread_suspend`. The GC path itself is covered end to end by the
/// golden suite (`tests/run/gc_stress.ty` in particular), which runs real
/// compiled programs.
#[cfg(not(test))]
fn raw_alloc(size: usize, atomic: bool) -> *mut u8 {
    unsafe {
        if atomic {
            GC_malloc_atomic(size) as *mut u8
        } else {
            GC_malloc(size) as *mut u8
        }
    }
}

#[cfg(test)]
fn raw_alloc(size: usize, _atomic: bool) -> *mut u8 {
    // Leaked on purpose: a test process is short lived, and a Typhoon value
    // is never freed explicitly.
    let layout = std::alloc::Layout::from_size_align(size.max(1), 8).expect("a valid layout");
    unsafe { std::alloc::alloc(layout) }
}

/// Allocates `size` GC-traced bytes. The block *is* scanned for pointers.
///
/// Aborts the process with `panic: out of memory` when the GC cannot satisfy
/// the request. Never returns null.
#[unsafe(no_mangle)]
pub extern "C" fn ty_alloc(size: usize) -> *mut u8 {
    let p = raw_alloc(size, false);
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
    let p = raw_alloc(size, true);
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

// ---------------------------------------------------------------------------
// Strings
// ---------------------------------------------------------------------------

/// A `str` value: a pointer to `{ len: i64, bytes: [u8; len] }` on the GC heap.
///
/// The header is what the compiler passes around; string literals are emitted
/// as private LLVM constants with exactly this layout, so a literal costs no
/// allocation at all. The payload holds no pointers, so it is allocated with
/// [`ty_alloc_atomic`] and never scanned.
///
/// The length is stored as an `i64` to match the `int` type and to keep the
/// bytes 8-byte aligned.
const STR_HEADER: usize = 8;

/// Allocates an uninitialised string of `len` bytes, returning the header.
///
/// # Safety
///
/// The caller must initialise all `len` payload bytes before the value is
/// handed to any other function.
unsafe fn str_alloc(len: usize) -> *mut u8 {
    let p = ty_alloc_atomic(STR_HEADER + len);
    unsafe { std::ptr::write_unaligned(p as *mut i64, len as i64) };
    p
}

/// Copies `bytes` into a fresh GC-allocated string.
pub fn str_new(bytes: &[u8]) -> *mut u8 {
    unsafe {
        let p = str_alloc(bytes.len());
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), p.add(STR_HEADER), bytes.len());
        p
    }
}

/// The payload of a string value.
///
/// # Safety
///
/// `s` must point at a well-formed string header.
pub unsafe fn str_bytes<'a>(s: *const u8) -> &'a [u8] {
    unsafe {
        let len = std::ptr::read_unaligned(s as *const i64) as usize;
        bytes(s.add(STR_HEADER), len)
    }
}

/// Concatenates two strings into a new one (`str` is immutable).
///
/// # Safety
///
/// Both arguments must point at well-formed string headers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ty_str_concat(a: *const u8, b: *const u8) -> *mut u8 {
    unsafe {
        let (a, b) = (str_bytes(a), str_bytes(b));
        let out = str_alloc(a.len() + b.len());
        std::ptr::copy_nonoverlapping(a.as_ptr(), out.add(STR_HEADER), a.len());
        std::ptr::copy_nonoverlapping(b.as_ptr(), out.add(STR_HEADER + a.len()), b.len());
        out
    }
}

/// Byte-wise string equality, `1` for equal.
///
/// # Safety
///
/// Both arguments must point at well-formed string headers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ty_str_eq(a: *const u8, b: *const u8) -> u8 {
    unsafe { u8::from(str_bytes(a) == str_bytes(b)) }
}

/// Renders an `int` the way `print` does.
#[unsafe(no_mangle)]
pub extern "C" fn ty_str_from_int(value: i64) -> *mut u8 {
    let mut buf = itoa(value);
    str_new(buf.as_bytes_mut())
}

/// Renders a `float` the way `print` does (shortest round-trip, always a `.`).
#[unsafe(no_mangle)]
pub extern "C" fn ty_str_from_float(value: f64) -> *mut u8 {
    str_new(format_float(value).as_bytes())
}

/// Renders a `bool` as `True` / `False`.
#[unsafe(no_mangle)]
pub extern "C" fn ty_str_from_bool(value: u8) -> *mut u8 {
    str_new(if value != 0 { b"True" } else { b"False" })
}

/// Renders a `float` with exactly `precision` digits after the point, the
/// `{x:.Nf}` format spec of DESIGN section 3.7.
///
/// Non-finite values ignore the precision and render as `inf`, `-inf`, `nan`,
/// which is what Python's `format` does.
#[unsafe(no_mangle)]
pub extern "C" fn ty_str_from_float_fixed(value: f64, precision: i64) -> *mut u8 {
    str_new(format_fixed(value, precision).as_bytes())
}

/// The text of `{value:.<precision>f}`.
pub fn format_fixed(value: f64, precision: i64) -> String {
    if !value.is_finite() {
        return format_float(value);
    }
    let precision = precision.clamp(0, 32) as usize;
    format!("{value:.precision$}")
}

// ---------------------------------------------------------------------------
// Integer power
// ---------------------------------------------------------------------------

/// `base ** exp` by squaring, wrapping like every other `int` operation
/// (DESIGN section 4.3); `None` for a negative exponent.
pub fn int_pow_checked(base: i64, exp: i64) -> Option<i64> {
    if exp < 0 {
        return None;
    }
    let mut base = base;
    let mut exp = exp;
    let mut acc: i64 = 1;
    while exp > 0 {
        if exp & 1 == 1 {
            acc = acc.wrapping_mul(base);
        }
        exp >>= 1;
        if exp > 0 {
            base = base.wrapping_mul(base);
        }
    }
    Some(acc)
}

/// `base ** exp` for two `int`s; panics on a negative exponent.
#[unsafe(no_mangle)]
pub extern "C" fn ty_int_pow(base: i64, exp: i64) -> i64 {
    match int_pow_checked(base, exp) {
        Some(value) => value,
        None => {
            let msg = b"negative exponent in integer power";
            unsafe { ty_panic(msg.as_ptr(), msg.len()) }
        }
    }
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
    fn allocation_returns_distinct_writable_blocks() {
        ty_rt_init();
        let mut blocks = Vec::new();
        for i in 1..=64usize {
            let size = i * 1024;
            let p = ty_alloc(size);
            assert!(!p.is_null());
            assert_eq!(p as usize % 8, 0, "blocks are at least 8-byte aligned");
            unsafe { std::ptr::write_bytes(p, 0xAB, size) };
            assert_eq!(unsafe { *p }, 0xAB);

            let q = ty_alloc_atomic(size);
            assert!(!q.is_null());
            unsafe { std::ptr::write_bytes(q, 0xCD, size) };
            assert_eq!(unsafe { *q.add(size - 1) }, 0xCD);

            blocks.push((p, size));
        }
        // Nothing overlaps, and the first write is still there.
        for (p, size) in &blocks {
            assert_eq!(unsafe { **p }, 0xAB);
            assert_eq!(unsafe { *p.add(size - 1) }, 0xAB);
        }
    }

    // -- str -----------------------------------------------------------

    /// Reads back a string value built by the runtime.
    fn read(s: *mut u8) -> String {
        String::from_utf8(unsafe { str_bytes(s) }.to_vec()).unwrap()
    }

    #[test]
    fn strings_round_trip_through_the_heap() {
        ty_rt_init();
        for text in ["", "a", "Hello, Typhoon!", "台风", "\0\u{1}"] {
            let s = str_new(text.as_bytes());
            assert_eq!(read(s), text);
            // The header holds the byte length, not the character count.
            assert_eq!(
                unsafe { std::ptr::read_unaligned(s as *const i64) },
                text.len() as i64
            );
        }
    }

    #[test]
    fn concatenation_allocates_a_new_string() {
        ty_rt_init();
        let a = str_new(b"Hello, ");
        let b = str_new("Typhoon 台风".as_bytes());
        let c = unsafe { ty_str_concat(a, b) };
        assert_eq!(read(c), "Hello, Typhoon 台风");
        // The operands are untouched: `str` is immutable.
        assert_eq!(read(a), "Hello, ");
        assert_eq!(read(b), "Typhoon 台风");

        let empty = str_new(b"");
        assert_eq!(read(unsafe { ty_str_concat(empty, empty) }), "");
        assert_eq!(read(unsafe { ty_str_concat(empty, a) }), "Hello, ");
        assert_eq!(read(unsafe { ty_str_concat(a, empty) }), "Hello, ");
    }

    #[test]
    fn string_equality_is_by_bytes() {
        ty_rt_init();
        let a = str_new(b"abc");
        let b = str_new(b"abc");
        let c = str_new(b"abd");
        let d = str_new(b"ab");
        assert_eq!(unsafe { ty_str_eq(a, b) }, 1);
        assert_eq!(unsafe { ty_str_eq(a, a) }, 1);
        assert_eq!(unsafe { ty_str_eq(a, c) }, 0);
        assert_eq!(unsafe { ty_str_eq(a, d) }, 0);
        let empty = str_new(b"");
        assert_eq!(unsafe { ty_str_eq(empty, empty) }, 1);
        assert_eq!(unsafe { ty_str_eq(empty, a) }, 0);
    }

    #[test]
    fn values_render_the_way_print_does() {
        ty_rt_init();
        assert_eq!(read(ty_str_from_int(-42)), "-42");
        assert_eq!(read(ty_str_from_int(i64::MIN)), "-9223372036854775808");
        assert_eq!(read(ty_str_from_float(5.0)), "5.0");
        assert_eq!(read(ty_str_from_float(1e16)), "1e+16");
        assert_eq!(read(ty_str_from_float(f64::NAN)), "nan");
        assert_eq!(read(ty_str_from_bool(1)), "True");
        assert_eq!(read(ty_str_from_bool(0)), "False");
    }

    #[test]
    fn fixed_precision_matches_python_format() {
        assert_eq!(format_fixed(3.5, 2), "3.50");
        assert_eq!(format_fixed(-0.0, 1), "-0.0");
        assert_eq!(format_fixed(2.675, 2), "2.67"); // the double is below 2.675
        assert_eq!(format_fixed(0.125, 2), "0.12"); // exact tie, rounds to even
        assert_eq!(format_fixed(0.375, 2), "0.38"); // exact tie, rounds to even
        assert_eq!(format_fixed(1.0, 0), "1");
        assert_eq!(format_fixed(1.5, 0), "2");
        assert_eq!(format_fixed(2.5, 0), "2");
        assert_eq!(format_fixed(1234.5678, 3), "1234.568");
        assert_eq!(format_fixed(f64::INFINITY, 2), "inf");
        assert_eq!(format_fixed(f64::NAN, 2), "nan");
        // Out-of-range precisions are clamped rather than panicking.
        assert_eq!(format_fixed(1.0, -3), "1");
        assert_eq!(format_fixed(1.0, 1000).len(), 34);
    }

    #[test]
    fn fixed_precision_allocates_a_string() {
        ty_rt_init();
        assert_eq!(read(ty_str_from_float_fixed(1.23456, 2)), "1.23");
        assert_eq!(read(ty_str_from_float_fixed(1.23456, 0)), "1");
    }

    // -- int_pow -------------------------------------------------------

    #[test]
    fn integer_power_by_squaring() {
        assert_eq!(int_pow_checked(2, 10), Some(1024));
        assert_eq!(int_pow_checked(3, 5), Some(243));
        assert_eq!(int_pow_checked(-2, 3), Some(-8));
        assert_eq!(int_pow_checked(-2, 4), Some(16));
        assert_eq!(int_pow_checked(7, 0), Some(1));
        assert_eq!(int_pow_checked(0, 0), Some(1));
        assert_eq!(int_pow_checked(0, 5), Some(0));
        assert_eq!(int_pow_checked(1, i64::MAX), Some(1));
        for base in [-5i64, -1, 0, 1, 2, 3, 10] {
            for exp in 0..12u32 {
                assert_eq!(
                    int_pow_checked(base, i64::from(exp)),
                    Some(base.wrapping_pow(exp)),
                    "{base} ** {exp}"
                );
            }
        }
    }

    #[test]
    fn integer_power_wraps_instead_of_trapping() {
        assert_eq!(int_pow_checked(2, 63), Some(i64::MIN));
        assert_eq!(int_pow_checked(2, 64), Some(0));
        assert_eq!(int_pow_checked(10, 19), Some(10i64.wrapping_pow(19)));
        assert_eq!(
            int_pow_checked(i64::MAX, 2),
            Some(i64::MAX.wrapping_mul(i64::MAX))
        );
    }

    #[test]
    fn a_negative_exponent_has_no_value() {
        // `ty_int_pow` turns this into `panic: negative exponent in integer
        // power`; the panic itself exits the process, so only the inner
        // function can be tested here.
        assert_eq!(int_pow_checked(2, -1), None);
        assert_eq!(int_pow_checked(0, i64::MIN), None);
        assert_eq!(ty_int_pow(2, 10), 1024);
    }

    #[test]
    fn zero_length_string_is_safe() {
        let s = unsafe { bytes(std::ptr::null(), 0) };
        assert!(s.is_empty());
    }
}
