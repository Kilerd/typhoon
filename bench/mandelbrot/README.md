# mandelbrot

Escape-time iteration over a `W x H` grid covering x in [-2, 1] and y in [-1, 1],
counting the points that never satisfy `x*x + y*y > 4.0` within `MAX_ITER` iterations.
Pure scalar float64: no allocation, no calls, a short data-dependent inner loop.

- Parameters: `W = H = 9000`, `MAX_ITER = 50` (81e6 points, up to 4.05e9 iterations).
- Expected output: `21467454`
- Go baseline: **1.514 s** (median of 5, Apple M4, go1.27.1).
- Every multiply that feeds an add is broken out into its own local (`t = 2.0 * x * y`
  then `y = t + cy`) so that neither Go's arm64 FMA contraction nor LLVM's can change
  the rounding: points sitting on the boundary of the set would otherwise flip and the
  two implementations would print different counts.
