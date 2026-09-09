# fib

Naive recursive `fib(N)`: no allocation, no floats, no loops — the benchmark is
almost entirely call/return overhead plus one integer add per call.

- Parameters: `N = 44` (~2.3e9 calls). DESIGN section 2 names `fib(35)`, but that
  finishes in under a millisecond on an Apple M4, so it is sized up here to keep the
  run in the 1-3 s band where timing noise is negligible.
- Expected output: `701408733`
- Go baseline: **1.438 s** (median of 5, Apple M4, go1.27.1).
