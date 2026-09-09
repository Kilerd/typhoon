# bench — typhoon vs Go

Every benchmark is one directory holding two implementations of the *same* algorithm
with the *same* loop structure: `main.ty` (typhoon) and `main.go` (Go). Both print the
same bytes, so the runner can diff their stdout; a ratio computed from two programs
that disagree is meaningless (DESIGN section 7.2).

The Go toolchain used for the baseline is pinned in [`GO_VERSION`](GO_VERSION)
(`go1.27.1`); update it and the numbers below together.

## Running

```sh
./run.sh                        # build + time everything, print the Markdown table
./run.sh --only nbody --runs 9  # one benchmark, 9 timed runs
./run.sh --no-typhoon           # Go baselines only (what you get before M1 lands)
./run.sh --json results.json    # also dump the raw timings
```

`run.sh` is a one-line wrapper around `run.py` (Python 3 standard library only, so the
`python3` that ships with macOS is enough). Useful options:

| option | meaning |
|---|---|
| `--runs N` | timed runs per binary, median reported (default 5, plus one warm-up run that is not timed) |
| `--only NAME` | restrict to one benchmark, repeatable |
| `--typhoon PATH` | compiler to use; default `target/release/typhoon`, then `target/debug/typhoon` |
| `--no-typhoon` | skip typhoon even if a compiler is present |
| `--go PATH` | Go toolchain to use; default the one on `$PATH` |
| `--tolerance F` | slack on every target before deciding PASS/FAIL, e.g. `0.05` for 1.05x |
| `--json FILE` | dump machine, versions, every run and the ratios |

Each Go program is built with `go build -o go_bin ./main.go` and each typhoon program
with `typhoon build -O2 main.ty -o ty_bin`, from inside the benchmark directory. Both
binaries are git-ignored. The runner exits non-zero if any ratio exceeds its target or
if the two implementations disagree on stdout — but only when typhoon actually ran;
with Go baselines alone there is nothing to pass or fail.

Build the compiler first (`cargo build --release -p typhoon`) so `run.sh` finds
`target/release/typhoon`; use `--no-typhoon` to just refresh the Go baselines.

`bench/go.mod` exists only so `go vet ./...` works inside a benchmark directory; the Go
programs import nothing outside `fmt` and `math`.

## Benchmarks

| benchmark | what it measures | parameters | expected output | target (DESIGN section 2) |
|---|---|---|---|---|
| `fib` | recursion / call overhead | `N = 44` | `701408733` | <= 1.0x Go (M1) |
| `loops` | integer loop codegen, `%` by a constant, wrapping i64 accumulator | `N = M = 60000` | `9256937139` | <= 1.0x Go (M1) |
| `mandelbrot` | scalar float loops with an early exit | `W = H = 9000`, `MAX_ITER = 50` | `21467454` | <= 1.0x Go (M1) |
| `nbody` | float arithmetic, `** 0.5`, long straight-line loop body | `N = 50000000`, `dt = 0.01` | `-0.169075164` / `-0.169059907` | <= 1.0x Go (M1) |
| `binary_trees` | allocator and GC: 202M two-pointer `class` nodes, nullable references | `MIN_DEPTH = 4`, `MAX_DEPTH = 18`, `SCALE = 3` | 10 lines, from `stretch tree of depth 19 check: 1048575` to `long lived tree of depth 18 check: 524287` | <= 2.0x Go (M2, Boehm phase) -> <= 1.0x (M5) |
| `fannkuch` | `list<int>` indexing and in-place swaps in tight loops | `N = 11` | `556355` / `Pfannkuchen(11) = 51` | <= 1.0x Go (M2) |
| `spectral_norm` | `list<float>` sequential reads, float divide, multiply-add | `N = 6650`, `ITERATIONS = 10` | `1.274224153` | <= 1.0x Go (M1 target, needs M2 lists) |

Sizes were chosen so Go takes 1-3 s on this machine: long enough that process startup
and scheduler noise are irrelevant, short enough that a full run of the suite is under
a minute. See each benchmark's own `README.md` for details.

The last three rows are the M2 benchmarks. DESIGN section 2 lists `spectral-norm` as an
M1 float target, but it needs `list<float>` (an n-element vector multiplied by an
implicit matrix), so it lands with M2 and keeps its `<= 1.0x` target; `nbody` and
`mandelbrot` carried the float target until then. `binary_trees` needs `class` and
nullable references, `fannkuch` needs `list<int>` and `tuple`.

## Go baseline

Measured with `./run.sh --runs 5 --no-typhoon` on:

- machine: `arm64` / Apple M4 (`uname -m` = `arm64`,
  `sysctl -n machdep.cpu.brand_string` = `Apple M4`), macOS 26.3 (Darwin 25.3.0)
- Go: `go1.27.1 darwin/arm64` (Homebrew, `/opt/homebrew/opt/go/bin/go`)

| benchmark | go (s), median of 5 | runs (s) |
|---|---|---|
| `fib` | **1.438** | 1.460, 1.438, 1.457, 1.432, 1.436 |
| `loops` | **1.513** | 1.508, 1.519, 1.511, 1.513, 1.516 |
| `mandelbrot` | **1.514** | 1.509, 1.510, 1.514, 1.515, 1.525 |
| `nbody` | **1.288** | 1.284, 1.289, 1.284, 1.339, 1.288 |
| `binary_trees` | **1.499** | 1.499, 1.491, 1.507, 1.497, 1.504 |
| `fannkuch` | **1.480** | 1.482, 1.477, 1.480, 1.477, 1.480 |
| `spectral_norm` | **1.510** | 1.510, 1.513, 1.509, 1.510, 1.507 |

Run-to-run spread is well under 1% for the loop benchmarks and ~4% for `fib`, so a
regression of more than a few percent is real. Note that the `<= 1.0x` target is a
knife edge: timing two identical binaries against each other on a laptop still lands
anywhere in 0.98x-1.02x, which is what `--tolerance` is for in CI.

## M2 results

Measured with `./run.sh --runs 7 --typhoon target/release/typhoon` on the same
machine, after `cargo build --release -p typhoon`:

| benchmark | go (s) | typhoon (s) | ratio ty/go | target | result |
|---|---|---|---|---|---|
| `fib` | 1.476 | 1.212 | 0.82x | <= 1.0x | PASS |
| `loops` | 1.599 | 1.669 | 1.04x | <= 1.0x | FAIL |
| `mandelbrot` | 1.528 | 1.507 | 0.99x | <= 1.0x | PASS |
| `nbody` | 1.323 | 0.897 | 0.68x | <= 1.0x | PASS |
| `binary_trees` | 1.567 | 1.667 | 1.06x | <= 2.0x | PASS |
| `fannkuch` | 1.549 | 1.540 | 0.99x | <= 1.0x | PASS |
| `spectral_norm` | 1.588 | 1.291 | 0.81x | <= 1.0x | PASS |

`loops` is the one row that misses its target, and it has missed it since M1: the
emitted IR is what `clang -O2` produces for the identical C loop, which is itself no
faster than Go's, so this is an LLVM-versus-`gc` codegen difference rather than a
lowering defect. It is tracked, not fixed.

`binary_trees` lands well inside its `<= 2.0x` Boehm-phase target because typhoon's
nodes are two words allocated with `GC_malloc` while Go's are two words plus a header
allocated from an mcache; the M5 escape-analysis work is what has to hold the line
when the target tightens to `<= 1.0x`.

`fannkuch` needed two codegen changes to reach parity, both in
`crates/codegen/src/emit/data.rs`: the emitter attaches TBAA metadata that separates a
list's `{len, cap, data}` header from its element buffer (without it LLVM must assume
`xs[i] = v` might overwrite the header, and re-loads `len` and `data` on every
iteration), and the bounds check puts the negative-index `+ len` fix-up in a cold
block so the fast path is the single unsigned compare Go also emits.

## Notes

- `main.ty` and `main.go` must stay algorithmically identical. If you change a
  parameter, change it in both, re-measure, and update the tables above.
- Every `main.ty` compiles and its stdout matches its `main.go` byte for byte; the
  runner checks that on every run, so a divergence fails the suite rather than
  quietly producing a meaningless ratio.
- `binary_trees` carries a `SCALE` constant on top of the classic tree count because
  that count is a pure power of two and `MAX_DEPTH` alone jumps from 1.00 s to 2.33 s
  on this machine; see `binary_trees/README.md` for how to get back to the canonical
  parameters.
- Both float benchmarks are sensitive to fused multiply-add. Go fuses `a*b + c` into
  one instruction on arm64; clang/LLVM only does with `-ffp-contract`. `mandelbrot` is
  written so neither can fuse (the count would differ). `nbody` is left in its
  canonical form: its printed 9 decimals are far coarser than the rounding difference,
  but the flop count is not, so if typhoon does not contract it will do strictly more
  work than Go here.
- `nbody` needs `d2 ** 0.5` to become a hardware `fsqrt`, not a libm `pow` call.
- `loops` relies on `% 7` being strength-reduced to a multiply/shift; if the ratio is
  ~10x, look at the division lowering first.
