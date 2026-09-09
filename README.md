# Typhoon

**Python's syntax, Go/Nim's semantics, a Rust toolchain, one static binary out.**
Typhoon takes the parts of Python that make code pleasant to read — indentation
blocks, `class`, `for x in ...`, f-strings — and puts entirely static semantics
underneath them. Every type is fixed at compile time, function signatures are
explicit, local variables are inferred, and there is no `Any`, no `eval`, no
monkey-patching and no runtime type dispatch. `typhoon build main.ty` produces a
single native executable with the runtime and garbage collector linked in: no
interpreter, no virtual machine, no `site-packages`. The performance target is
not "faster than Python" — it is parity with Go on single-threaded benchmarks.
Typhoon deliberately borrows Python's *syntax only*, not its semantics and not
its ecosystem; see [`docs/DESIGN.md`](docs/DESIGN.md) for the full design.

```python
fn fib(n: int) -> int:
    if n < 2:
        return n
    return fib(n - 1) + fib(n - 2)

fn main():
    print(fib(30))
    for i in range(5):
        print(fib(i))
```

## Status

Typhoon is under active development and is **not usable yet**. Progress is
tracked by milestone; each one has a measurable exit criterion
(`docs/DESIGN.md` §8).

| Milestone | Scope | Exit criterion | Status |
|---|---|---|---|
| **M0** Reset | Workspace skeleton, toolchain, runtime + Boehm GC linkage, golden test harness | `hello.ty` compiles and runs in CI | **In progress** |
| M1 Numeric core | lexer/parser, `fn`, `int`/`float`/`bool`, operators, `if`/`while`/`for range`, recursion, `print`, type checking | fib / nbody / mandelbrot / spectral-norm ≤ 1.0x Go | Not started |
| M2 Data | `class`, `str`, `list<T>`, `tuple`, `for-in` | binary-trees ≤ 2.0x, fannkuch ≤ 1.0x | Not started |
| M3 Generics & inference | Monomorphised generics, `dict`/`set`, comprehensions, `T \| None`, comparison chains | k-nucleotide ≤ 1.5x | Not started |
| M4 Errors & modules | `try`/`except`/`raise`, `import`, C FFI | fasta / k-nucleotide ≤ 1.0x | Not started |
| M5 Performance & UX | Escape analysis, GC replacement study, JIT `run`, diagnostics, formatter | binary-trees ≤ 1.0x | Not started |
| M6 Concurrency | Model undecided | TBD | Not started |

The backend, runtime, driver, CLI and test harness exist today; the frontend
(`lexer` → `parser` → `sema` → `codegen`) is being built, so every program
currently fails with `frontend not implemented yet`. The golden case
`examples/hello` is red on purpose — turning it green *is* the M0 exit criterion.

## Building

Typhoon shells out to the system `clang` to optimise and link the LLVM IR it
emits, and links Boehm GC into every binary, so both must be installed.

```sh
# macOS
brew install llvm bdw-gc go        # go is only used for the benchmark baselines

# Debian / Ubuntu
sudo apt-get install -y clang llvm libgc-dev golang-go
```

Then:

```sh
cargo build --workspace
cargo test  --workspace
```

`cargo test --workspace` runs the unit tests, the driver's link-and-run tests
against hand-written LLVM IR fixtures, and the golden harness described below.

## Using the CLI

```sh
typhoon build   main.ty [-o OUT] [-O 0..3] [--emit-ir] [--keep-temps]
typhoon run     main.ty [-O 0..3] [-- ARGS...]
typhoon emit-ir main.ty [-o OUT]
typhoon check   main.ty
```

| Command | Does |
|---|---|
| `build` | Compiles to a native executable (defaults to the source file's stem) |
| `run` | Compiles to a temporary directory, runs it, and propagates its exit code |
| `emit-ir` | Stops after codegen and prints the textual LLVM IR (`docs/DESIGN.md` §6.3) |
| `check` | Runs the frontend only; prints diagnostics and produces no binary |

Exit codes: `0` success, `1` compile or link error (diagnostics on stderr),
`2` usage error — including an input file that cannot be read.

## Environment variables

The driver locates its external pieces automatically; every lookup can be
overridden.

| Variable | Overrides | Default |
|---|---|---|
| `TYPHOON_CLANG` | The `clang` used to optimise and link | `clang` from `PATH` |
| `TYPHOON_RUNTIME_LIB` | Path of `libtyphoon_runtime.a` | Searched next to the running executable and one directory up (covers `target/debug/` and `target/debug/deps/`) |
| `TYPHOON_GC_LIB_DIR` | Directory holding Boehm GC (`libgc`) | `brew --prefix bdw-gc`/lib, then the usual system library directories |
| `TYPHOON_MILESTONE` | Milestone the golden harness enforces | The `CURRENT_MILESTONE` constant in `crates/driver/tests/golden.rs` |

## Tests

Golden tests are plain Typhoon programs that declare their own expectations in
comments (`docs/DESIGN.md` §7.1). Each file becomes its own named test case.

| Directory | Must | Declares |
|---|---|---|
| `tests/run/*.ty` | Compile, run, exit 0 | `# expect: <line>` — one per line of stdout, in order, compared exactly |
| `examples/*.ty` | Compile, run, exit 0 | Same as `tests/run` — so the documentation examples can never rot |
| `tests/fail/*.ty` | Fail to compile | `# error: <substring>` — every substring must appear in the diagnostics |

Every file also declares `# milestone: M0` … `M6`. Cases above the harness's
current milestone are reported as **ignored** rather than failed, so the suite
can carry future milestones' cases from day one:

```sh
cargo test --workspace                          # enforce the current milestone
TYPHOON_MILESTONE=M1 cargo test -p typhoon-driver --test golden
cargo test -p typhoon-driver --test golden -- --ignored   # run the future cases
```

## Repository layout

| Path | Contents |
|---|---|
| `crates/diag` | Spans, diagnostics and the renderer |
| `crates/lexer` | Indentation-aware lexer (`INDENT`/`DEDENT`/`NEWLINE`) |
| `crates/ast` | AST data structures |
| `crates/parser` | Hand-written recursive-descent + Pratt parser |
| `crates/sema` | Name resolution, type inference and checking, monomorphisation |
| `crates/codegen` | Typed AST → textual LLVM IR |
| `crates/runtime` | `staticlib` runtime: `ty_alloc`, `ty_print_*`, `ty_panic` |
| `crates/driver` | Compile → link → run pipeline, plus the golden test harness |
| `crates/cli` | The `typhoon` binary |
| `examples/` | Runnable, tested example programs |
| `docs/DESIGN.md` | The authoritative design document |

## License

See [LICENSE](LICENSE).
