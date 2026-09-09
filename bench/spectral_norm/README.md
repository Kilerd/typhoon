# spectral_norm

The Benchmarks Game eigenvalue benchmark: ten power-method iterations of `AtA*u` over
`N`-element vectors, where `A` is never materialised but evaluated on the fly as
`A[i][j] = 1/((i+j)*(i+j+1)/2 + i + 1)`, followed by the Rayleigh quotient
`sqrt(vBv/vv)`. The inner loop is a sequential indexed read of a `list<float>` plus one
integer division, one float division and one multiply-add per element — the float
counterpart to `loops`, and the benchmark that keeps `list<float>` honest.

- Parameters: `N = 6650`, `ITERATIONS = 10`, i.e. `40 * N^2` = 1.77e9 evaluations of
  `eval_a` and as many multiply-adds. Three vectors of 6650 floats (`u`, `v` and the
  scratch `w`) are grown once with `append` before the timed work.
- Expected output: `1.274224153` — the canonical spectral-norm value, printed with
  `%.9f` on the Go side and `f"{result:.9f}"` on the typhoon side.
- Go baseline: **1.510 s** (median of 5, Apple M4, go1.27.1).
- DESIGN section 2 lists spectral-norm as an *M1* float target, but it needs
  `list<float>`, which arrives with M2; it joins the suite here and keeps the `<= 1.0x`
  M1 target.
- `/` on two `int`s yields a `float` in typhoon (DESIGN 4.3), so `eval_a` uses `//` for
  `(i+j)*(i+j+1)//2` and an explicit `float(...)` for the conversion, matching Go's
  `float64((i+j)*(i+j+1)/2+i+1)`. Both operands are non-negative and the product is
  always even, so floor and truncating division cannot differ here.
- Unlike `mandelbrot`, this benchmark is written in its canonical form and lets both
  compilers contract `total + eval_a(i, j)*u[j]` into an FMA. The result is a smooth
  float, not a count: a last-bit rounding difference moves the value by ~1e-16
  relative, six orders of magnitude below the ninth printed decimal.
