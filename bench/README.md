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

Until the M1 compiler exists, `run.sh` finds `target/debug/typhoon`, fails to build
`main.ty` and says so; use `--no-typhoon` to just refresh the Go baselines.

`bench/go.mod` exists only so `go vet ./...` works inside a benchmark directory; the Go
programs import nothing outside `fmt` and `math`.

## Benchmarks

| benchmark | what it measures | parameters | expected output | target (DESIGN section 2) |
|---|---|---|---|---|
| `fib` | recursion / call overhead | `N = 44` | `701408733` | <= 1.0x Go (M1) |
| `loops` | integer loop codegen, `%` by a constant, wrapping i64 accumulator | `N = M = 60000` | `9256937139` | <= 1.0x Go (M1) |
| `mandelbrot` | scalar float loops with an early exit | `W = H = 9000`, `MAX_ITER = 50` | `21467454` | <= 1.0x Go (M1) |
| `nbody` | float arithmetic, `** 0.5`, long straight-line loop body | `N = 50000000`, `dt = 0.01` | `-0.169075164` / `-0.169059907` | <= 1.0x Go (M1) |

Sizes were chosen so Go takes 1-3 s on this machine: long enough that process startup
and scheduler noise are irrelevant, short enough that a full run of the suite is under
a minute. See each benchmark's own `README.md` for details.

**`spectral-norm` is missing on purpose.** DESIGN section 2 lists it as an M1 float
target, but it needs `list<float>` (an n-element vector multiplied by an implicit
matrix), and M1 has no lists. It joins the suite at M2 together with `list<float>`;
until then `nbody` and `mandelbrot` carry the float target.

## M1 Go baseline

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

Run-to-run spread is well under 1% for the loop benchmarks and ~4% for `fib`, so a
regression of more than a few percent is real. Note that the `<= 1.0x` target is a
knife edge: timing two identical binaries against each other on a laptop still lands
anywhere in 0.98x-1.02x, which is what `--tolerance` is for in CI.

## Notes for whoever lands M1

- `main.ty` and `main.go` must stay algorithmically identical. If you change a
  parameter, change it in both, re-measure, and update the tables above.
- The typhoon programs have never been compiled (they were written against DESIGN
  sections 3 and 4 before the frontend existed). Expect to fix small things; the
  expected outputs above come from the Go versions and are the source of truth.
- Both float benchmarks are sensitive to fused multiply-add. Go fuses `a*b + c` into
  one instruction on arm64; clang/LLVM only does with `-ffp-contract`. `mandelbrot` is
  written so neither can fuse (the count would differ). `nbody` is left in its
  canonical form: its printed 9 decimals are far coarser than the rounding difference,
  but the flop count is not, so if typhoon does not contract it will do strictly more
  work than Go here.
- `nbody` needs `d2 ** 0.5` to become a hardware `fsqrt`, not a libm `pow` call.
- `loops` relies on `% 7` being strength-reduced to a multiply/shift; if the ratio is
  ~10x, look at the division lowering first.
