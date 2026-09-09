# loops

Nested integer loops accumulating `(i * j) % 7` into a wrapping i64. Measures
induction-variable lowering, `%` by a constant (should become multiply/shift, not a
divide) and keeping the accumulator in a register across the inner loop.

- Parameters: `N = M = 60000`, i.e. 3.6e9 inner iterations. `i` and `j` are always
  non-negative, so typhoon's floor `%` and Go's truncating `%` agree.
- The accumulator is an `int`, i.e. an i64 that is *defined* to wrap (DESIGN 4.3) so no
  overflow check lands in the inner loop; at this size the sum stays below 2^63, so
  both implementations print the same positive number.
- Expected output: `9256937139`
- Go baseline: **1.513 s** (median of 5, Apple M4, go1.27.1).
