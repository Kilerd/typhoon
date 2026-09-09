# nbody

The Benchmarks Game 5-body integrator (sun + Jupiter, Saturn, Uranus, Neptune) with
the standard constants and initial values, momentum offset first, total energy printed
with 9 decimals before and after the integration. M1 has no lists and no classes, so
the system lives in 35 scalar locals and the 10 pairwise interactions are unrolled by
hand; the Go version is written the same way so the comparison is fair.

- Parameters: `N = 50000000` steps, `dt = 0.01` (5e8 pairwise interactions).
- Expected output: `-0.169075164` then `-0.169059907` — the canonical n=50,000,000
  values, reproduced exactly by this Go version. Note: at n=1,000,000 the second line
  is `-0.169086185`, *not* `-0.169059907`.
- Go baseline: **1.288 s** (median of 5, Apple M4, go1.27.1).
- M1 has no `sqrt` builtin, so the distance is written `d2 ** 0.5` (as in
  `examples/nbody.ty`). Go uses `math.Sqrt`, which is a single `fsqrt` instruction; for
  this benchmark to mean anything, typhoon must lower `x ** 0.5` to `fsqrt` too and not
  to a libm `pow` call.
