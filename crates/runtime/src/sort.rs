//! `sorted(xs)` (DESIGN §4.6): a sorted copy of a list.
//!
//! The element type is described by the same descriptor a dict key is (see
//! [`crate::dict::KeyShape`]), extended with `float`: a scalar, or a flat tuple
//! compared member by member, left to right — so `sorted(pairs)` over
//! `(-count, name)` tuples orders by count, then by name, exactly as Python
//! does. The sort is stable.
//!
//! `str` compares byte-wise, which for UTF-8 is code point order (Python's).
//! `float` uses `<`, with NaN placed after every number: Python's own result
//! with a NaN in the list depends on where it sits, and a comparator that is
//! not a total order is not something to hand to a sort.

use std::cmp::Ordering;

use crate::dict::{KEY_BOOL, KEY_FLOAT, KEY_INT, KEY_STR, KeyShape};
use crate::{TyList, str_bytes, ty_list_new};

/// Python's order on two floats, made total by putting NaN last.
pub fn float_order(a: f64, b: f64) -> Ordering {
    match a.partial_cmp(&b) {
        Some(order) => order,
        None => a.is_nan().cmp(&b.is_nan()),
    }
}

/// Member-wise comparison of two values laid out as `shape` describes.
///
/// # Safety
///
/// Both pointers must point at such values, with well-formed `str` members.
unsafe fn compare(shape: &KeyShape, a: *const u8, b: *const u8) -> Ordering {
    for (i, offset) in shape.offsets().iter().enumerate() {
        let offset = usize::from(*offset);
        let kind = shape.kind(i);
        let order = unsafe {
            let (x, y) = (a.add(offset), b.add(offset));
            match kind {
                KEY_BOOL => (*x).cmp(&*y),
                KEY_STR => {
                    let (x, y) = (
                        std::ptr::read_unaligned(x as *const *const u8),
                        std::ptr::read_unaligned(y as *const *const u8),
                    );
                    str_bytes(x).cmp(str_bytes(y))
                }
                KEY_FLOAT => float_order(
                    std::ptr::read_unaligned(x as *const f64),
                    std::ptr::read_unaligned(y as *const f64),
                ),
                _ => std::ptr::read_unaligned(x as *const i64)
                    .cmp(&std::ptr::read_unaligned(y as *const i64)),
            }
        };
        if order != Ordering::Equal {
            return order;
        }
    }
    Ordering::Equal
}

/// `sorted(xs)`: a fresh list holding `xs`'s elements in ascending order.
///
/// `desc` describes the element type (a scalar or a flat tuple of `int`,
/// `float`, `bool` and `str`); its size is the list's element size.
///
/// # Safety
///
/// `list` must point at a well-formed [`TyList`] whose elements are laid out
/// as `desc` describes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ty_list_sorted(list: *const TyList, desc: i64) -> *mut TyList {
    let shape = KeyShape::of(desc as u64);
    let size = shape.size;
    unsafe {
        let n = (*list).len.max(0) as usize;
        let out = ty_list_new(size as i64, u8::from(!shape.has_pointers), n as i64);
        if n == 0 {
            return out;
        }
        let src = (*list).data as *const u8;
        let dst = (*out).data;
        match desc as u64 {
            // The common cases sort the values themselves, not an index.
            KEY_INT => {
                let mut values: Vec<i64> = (0..n)
                    .map(|i| std::ptr::read_unaligned(src.add(i * 8) as *const i64))
                    .collect();
                values.sort_unstable();
                for (i, v) in values.iter().enumerate() {
                    std::ptr::write_unaligned(dst.add(i * 8) as *mut i64, *v);
                }
            }
            KEY_STR => {
                let mut values: Vec<*const u8> = (0..n)
                    .map(|i| std::ptr::read_unaligned(src.add(i * size) as *const *const u8))
                    .collect();
                values.sort_by(|a, b| str_bytes(*a).cmp(str_bytes(*b)));
                for (i, v) in values.iter().enumerate() {
                    std::ptr::write_unaligned(dst.add(i * size) as *mut *const u8, *v);
                }
            }
            _ => {
                let mut order: Vec<usize> = (0..n).collect();
                order.sort_by(|a, b| compare(&shape, src.add(a * size), src.add(b * size)));
                for (i, from) in order.iter().enumerate() {
                    std::ptr::copy_nonoverlapping(src.add(from * size), dst.add(i * size), size);
                }
            }
        }
        (*out).len = n as i64;
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::str_new;

    unsafe fn list_of_i64(values: &[i64]) -> *mut TyList {
        let list = ty_list_new(8, 1, values.len() as i64);
        unsafe {
            for (i, v) in values.iter().enumerate() {
                std::ptr::write_unaligned((*list).data.add(i * 8) as *mut i64, *v);
            }
            (*list).len = values.len() as i64;
        }
        list
    }

    unsafe fn read_i64s(list: *const TyList) -> Vec<i64> {
        unsafe {
            (0..(*list).len as usize)
                .map(|i| std::ptr::read_unaligned((*list).data.add(i * 8) as *const i64))
                .collect()
        }
    }

    #[test]
    fn ints_sort_ascending_into_a_copy() {
        unsafe {
            let list = list_of_i64(&[3, -1, 2, 2, 0]);
            let sorted = ty_list_sorted(list, KEY_INT as i64);
            assert_eq!(read_i64s(sorted), [-1, 0, 2, 2, 3]);
            assert_eq!(read_i64s(list), [3, -1, 2, 2, 0]);
            let empty = ty_list_sorted(list_of_i64(&[]), KEY_INT as i64);
            assert_eq!((*empty).len, 0);
        }
    }

    #[test]
    fn floats_put_nan_last_and_keep_equal_zeros_stable() {
        let mut values = [f64::NAN, 1.0, -0.0, 0.0, -2.5];
        values.sort_by(|a, b| float_order(*a, *b));
        assert_eq!(values[0], -2.5);
        assert!(values[1].is_sign_negative() && values[1] == 0.0);
        assert!(values[2].is_sign_positive() && values[2] == 0.0);
        assert!(values[4].is_nan());
    }

    #[test]
    fn tuples_compare_member_by_member() {
        unsafe {
            // tuple<int, str>: 16 bytes on a 64-bit target.
            let desc = (KEY_INT | KEY_STR << 4) as i64;
            let size = KeyShape::of(desc as u64).size;
            let rows: [(i64, &str); 4] = [(-2, "b"), (-3, "z"), (-2, "a"), (0, "")];
            let list = ty_list_new(size as i64, 0, 4);
            for (i, (n, s)) in rows.iter().enumerate() {
                let at = (*list).data.add(i * size);
                std::ptr::write_unaligned(at as *mut i64, *n);
                std::ptr::write_unaligned(at.add(8) as *mut *const u8, str_new(s.as_bytes()));
            }
            (*list).len = 4;
            let sorted = ty_list_sorted(list, desc);
            let got: Vec<(i64, Vec<u8>)> = (0..4)
                .map(|i| {
                    let at = (*sorted).data.add(i * size);
                    (
                        std::ptr::read_unaligned(at as *const i64),
                        str_bytes(std::ptr::read_unaligned(at.add(8) as *const *const u8)).to_vec(),
                    )
                })
                .collect();
            assert_eq!(
                got,
                [
                    (-3, b"z".to_vec()),
                    (-2, b"a".to_vec()),
                    (-2, b"b".to_vec()),
                    (0, b"".to_vec())
                ]
            );
        }
    }
}
