//! Codegen tests: snapshots of the emitted LLVM IR, and a check that every IR
//! module the compiler can produce for the repository's programs is accepted
//! by LLVM itself.

use std::path::{Path, PathBuf};
use std::process::Command;

use typhoon_codegen::compile_to_llvm_ir;

/// Compiles `src`, panicking with the rendered diagnostics on failure.
#[track_caller]
fn ir(src: &str) -> String {
    compile_to_llvm_ir("main.ty", src).unwrap_or_else(|errors| panic!("{}", errors.join("\n")))
}

/// The workspace root.
fn repo_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/codegen always has two ancestors")
}

// ---------------------------------------------------------------------------
// Snapshots
// ---------------------------------------------------------------------------

macro_rules! snapshot_ir {
    ($name:ident, $src:expr) => {
        #[test]
        fn $name() {
            insta::assert_snapshot!(ir($src));
        }
    };
}

snapshot_ir!(
    hello_and_the_entry_point,
    "fn main():\n    print(\"Hello, Typhoon!\")\n"
);

snapshot_ir!(
    recursive_fib,
    "fn fib(n: int) -> int:\n    if n < 2:\n        return n\n    return fib(n - 1) + fib(n - 2)\n\nfn main():\n    print(fib(10))\n"
);

snapshot_ir!(
    integer_floor_division_and_modulo,
    "fn divide(a: int, b: int) -> int:\n    return a // b\n\nfn main():\n    print(divide(-7, 2))\n    print(-7 % 2)\n    print(7 / 2)\n"
);

snapshot_ir!(
    float_arithmetic_is_contracted,
    "fn main():\n    a = 1.5\n    b = 2.5\n    print(a * b + a)\n    print(a - b)\n    print(a / b)\n    print(float(a) * b + a)\n"
);

snapshot_ir!(
    float_floor_division_and_modulo,
    "fn main():\n    a = 7.0\n    b = 2.0\n    print(a // b)\n    print(a % b)\n"
);

snapshot_ir!(
    powers_use_sqrt_multiply_and_the_runtime,
    "fn main():\n    x = 2.0\n    n = 3\n    print(x ** 0.5)\n    print(x ** 2.0)\n    print(x ** 3.0)\n    print(n ** 2)\n    print(n ** n)\n"
);

snapshot_ir!(
    short_circuit_operators,
    "fn side(n: int) -> bool:\n    print(n)\n    return n > 0\n\nfn main():\n    print(side(1) and side(2))\n    print(side(0) or side(3))\n"
);

snapshot_ir!(
    for_range_is_a_counting_loop,
    "fn main():\n    total = 0\n    for i in range(10):\n        total = total + i\n    for j in range(10, 0, -2):\n        total = total + j\n    print(total)\n"
);

snapshot_ir!(
    while_with_break_and_continue,
    "fn main():\n    i = 0\n    while i < 10:\n        i = i + 1\n        if i == 3:\n            continue\n        if i > 5:\n            break\n        print(i)\n"
);

snapshot_ir!(
    f_strings_build_a_string,
    "fn main():\n    n = 2\n    x = 1.5\n    print(f\"n={n} x={x:.2f} {{literal}}\")\n"
);

snapshot_ir!(
    string_concatenation_and_equality,
    "fn main():\n    a = \"Hello, \"\n    b = a + \"Typhoon!\"\n    print(b)\n    print(b == \"Hello, Typhoon!\")\n    print(b != a)\n"
);

snapshot_ir!(
    keyword_arguments_and_defaults,
    "fn scale(x: float, factor: float = 2.0) -> float:\n    return x * factor\n\nfn main():\n    print(scale(1.0))\n    print(scale(factor=3.0, x=1.0))\n"
);

snapshot_ir!(
    constants_are_folded,
    "LIMIT: int = 2 * 5\nNAME: str = \"ty\" + \"phoon\"\n\nfn main():\n    for i in range(LIMIT):\n        print(i)\n    print(NAME)\n"
);

snapshot_ir!(
    printing_every_type,
    "fn main():\n    print(1, 1.5, True, \"s\")\n    print()\n"
);

snapshot_ir!(
    bitwise_operations_and_shifts,
    "fn main():\n    n = 5\n    k = 2\n    print(n & 3, n | 8, n ^ 1, ~n)\n    print(n << 2, n >> 1)\n    print(n << k)\n"
);

snapshot_ir!(
    builtins_lower_to_intrinsics,
    "fn main():\n    n = -3\n    x = -1.5\n    print(abs(n), abs(x))\n    print(min(1, 2), max(1.0, 2.0))\n    print(float(n), int(x))\n"
);

snapshot_ir!(
    a_unit_function_returns_void,
    "fn log(n: int):\n    if n < 0:\n        return\n    print(n)\n\nfn main():\n    log(1)\n    log(-1)\n"
);

// ---------------------------------------------------------------------------
// Every generated module must be valid LLVM IR
// ---------------------------------------------------------------------------

/// How to check that a `.ll` file parses: `llvm-as` when it is available,
/// otherwise `clang -c -x ir`, which every machine that can link a Typhoon
/// binary has anyway.
enum Verifier {
    LlvmAs(PathBuf),
    Clang(PathBuf),
}

impl Verifier {
    fn find() -> Verifier {
        let candidates = [
            std::env::var_os("TYPHOON_LLVM_AS").map(PathBuf::from),
            Some(PathBuf::from("/opt/homebrew/opt/llvm/bin/llvm-as")),
            Some(PathBuf::from("/usr/local/opt/llvm/bin/llvm-as")),
        ];
        for candidate in candidates.into_iter().flatten() {
            if candidate.is_file() {
                return Verifier::LlvmAs(candidate);
            }
        }
        if let Ok(out) = Command::new("llvm-as").arg("--version").output()
            && out.status.success()
        {
            return Verifier::LlvmAs(PathBuf::from("llvm-as"));
        }
        let clang = std::env::var_os("TYPHOON_CLANG")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("clang"));
        Verifier::Clang(clang)
    }

    /// Returns the tool's complaints, or `None` when the module is valid.
    fn check(&self, path: &Path) -> Option<String> {
        let out = match self {
            Verifier::LlvmAs(tool) => Command::new(tool)
                .arg(path)
                .arg("-o")
                .arg(devnull())
                .output(),
            Verifier::Clang(tool) => Command::new(tool)
                .args(["-Wno-override-module", "-c", "-x", "ir"])
                .arg(path)
                .arg("-o")
                .arg(devnull())
                .output(),
        };
        let out = out.unwrap_or_else(|err| panic!("cannot run the IR verifier: {err}"));
        if out.status.success() {
            return None;
        }
        Some(format!(
            "{}{}",
            String::from_utf8_lossy(&out.stderr),
            String::from_utf8_lossy(&out.stdout)
        ))
    }
}

fn devnull() -> PathBuf {
    PathBuf::from(if cfg!(windows) { "NUL" } else { "/dev/null" })
}

/// The milestone a golden file declares, or 0.
fn milestone_of(source: &str) -> u32 {
    for line in source.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("# milestone:") {
            return rest
                .trim()
                .trim_start_matches(['M', 'm'])
                .parse()
                .unwrap_or(0);
        }
    }
    0
}

/// Every `.ty` file of the repository that M1 is expected to compile.
fn m1_sources() -> Vec<(String, String)> {
    let mut out = Vec::new();
    for dir in ["examples", "tests/run"] {
        let path = repo_root().join(dir);
        let mut entries: Vec<PathBuf> = std::fs::read_dir(&path)
            .unwrap_or_else(|err| panic!("cannot read {}: {err}", path.display()))
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "ty"))
            .collect();
        entries.sort();
        for entry in entries {
            let source = std::fs::read_to_string(&entry).unwrap();
            if milestone_of(&source) > 1 {
                continue;
            }
            out.push((
                format!("{dir}/{}", entry.file_name().unwrap().to_string_lossy()),
                source,
            ));
        }
    }
    out
}

#[test]
fn every_repository_program_compiles_to_valid_llvm_ir() {
    let verifier = Verifier::find();
    let dir = std::env::temp_dir().join(format!("typhoon-ir-check-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    let sources = m1_sources();
    assert!(
        sources.len() >= 20,
        "expected the repository to carry at least 20 M0/M1 programs, found {}",
        sources.len()
    );

    let mut problems = Vec::new();
    for (name, source) in &sources {
        let text = match compile_to_llvm_ir(name, source) {
            Ok(text) => text,
            Err(errors) => {
                problems.push(format!("{name} did not compile:\n{}", errors.join("\n")));
                continue;
            }
        };
        let path = dir.join(name.replace('/', "_")).with_extension("ll");
        std::fs::write(&path, &text).unwrap();
        if let Some(complaint) = verifier.check(&path) {
            problems.push(format!("{name} is not valid LLVM IR:\n{complaint}"));
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert!(problems.is_empty(), "{}", problems.join("\n\n"));
}

#[test]
fn the_ir_is_deterministic() {
    let source = "LIMIT: int = 3\n\nfn main():\n    for i in range(LIMIT):\n        print(f\"{i}\", \"x\", 1.0)\n";
    assert_eq!(ir(source), ir(source));
}

#[test]
fn every_declared_runtime_symbol_is_used_and_declared_once() {
    let text = ir(
        "fn main():\n    print(1, \"a\" + \"b\", 2.0, True)\n    print(f\"{1}{2.0}{True}{1:d}{2.0:.2f}\")\n",
    );
    let declarations: Vec<&str> = text.lines().filter(|l| l.starts_with("declare ")).collect();
    let mut symbols: Vec<&str> = declarations
        .iter()
        .map(|line| {
            line.split('@')
                .nth(1)
                .and_then(|rest| rest.split('(').next())
                .expect("a declaration names a symbol")
        })
        .collect();
    assert!(
        symbols.len() > 5,
        "expected several runtime declarations:\n{text}"
    );
    for symbol in &symbols {
        assert!(
            text.contains(&format!("call void @{symbol}("))
                || text.contains(&format!("@{symbol}(i"))
                || text.contains(&format!("@{symbol}(double"))
                || text.contains(&format!("@{symbol}(ptr")),
            "{symbol} is declared but never called:\n{text}"
        );
    }
    let before = symbols.len();
    symbols.sort_unstable();
    symbols.dedup();
    assert_eq!(before, symbols.len(), "a symbol is declared twice:\n{text}");
}

#[test]
fn nothing_is_declared_that_is_not_needed() {
    // `print` of an int must not drag in the string or float runtime.
    let text = ir("fn main():\n    print(1)\n");
    assert!(text.contains("declare void @ty_print_int(i64)"), "{text}");
    assert!(!text.contains("ty_str_concat"), "{text}");
    assert!(!text.contains("ty_print_float"), "{text}");
    assert!(!text.contains("llvm."), "{text}");
}

// ---------------------------------------------------------------------------
// Every M2 construct must produce valid LLVM IR too
// ---------------------------------------------------------------------------

/// Short programs covering every construct M2 adds (DESIGN 3.8, 4.1, 4.2,
/// 4.6). The repository's own M2 programs are checked by the golden harness;
/// this corpus is spelled out here because this is the test that catches
/// malformed IR, so its breadth is the point.
const M2_PROGRAMS: &[(&str, &str)] = &[
    (
        "class with methods",
        r#"class Point:
    x: float
    y: float = 0.0

    fn len2(self) -> float:
        return self.x * self.x + self.y * self.y

    fn norm(self) -> float:
        return self.len2() ** 0.5

    fn shift(self, dx: float):
        self.x = self.x + dx

fn main():
    p = Point(x=3.0, y=4.0)
    print(p.norm())
    p.shift(1.0)
    print(p.x, p.y)
    q = Point(x=3.0)
    print(p is q, p is not q)
"#,
    ),
    (
        "class with a pointer field",
        r#"class Row:
    name: str
    counts: list<int>

    fn total(self) -> int:
        sum = 0
        for n in self.counts:
            sum = sum + n
        return sum

fn main():
    r = Row(name="a", counts=[1, 2, 3])
    r.counts.append(4)
    print(r.name, r.total(), r.counts)
"#,
    ),
    (
        "optional narrowed in an if",
        r#"class Node:
    value: int

fn main():
    n: Node | None = Node(value=1)
    if n is None:
        print("empty")
    else:
        print(n.value)
    m: Node | None = None
    if m is not None:
        print(m.value)
    print(m is None)
"#,
    ),
    (
        "optional narrowed by an early return",
        r#"class Node:
    value: int
    next: Node | None = None

fn head(n: Node | None) -> int:
    if n is None:
        return -1
    return n.value

fn main():
    tail = Node(value=2)
    first = Node(value=1, next=tail)
    print(head(first))
    print(head(None))
    print(head(first.next))
"#,
    ),
    (
        "optional field walked with a local",
        r#"class Node:
    value: int
    next: Node | None = None

fn main():
    third = Node(value=3)
    second = Node(value=2, next=third)
    first = Node(value=1, next=second)
    cur: Node | None = first
    for i in range(3):
        if cur is not None:
            print(cur.value)
            step: Node | None = cur.next
            cur = step
"#,
    ),
    (
        "list literal index and store",
        r#"fn main():
    xs = [1, 2, 3]
    i = 1
    print(xs[i], xs[-1])
    xs[0] = 9
    xs[-1] = 8
    print(xs)
"#,
    ),
    (
        "list append pop insert clear",
        r#"fn main():
    xs: list<int> = []
    for i in range(5):
        xs.append(i * i)
    xs.insert(0, -1)
    print(xs.pop(), len(xs))
    xs.clear()
    print(len(xs), xs)
"#,
    ),
    (
        "list concat membership and equality",
        r#"fn main():
    xs = [1, 2]
    ys = [3]
    both = xs + ys
    print(both, 3 in both, 4 not in both)
    print(both == [1, 2, 3], both != xs)
    names = ["a", "b"]
    print("a" in names, names == ["a", "b"])
"#,
    ),
    (
        "nested lists",
        r#"fn main():
    grid = [[1, 2], [3]]
    grid[0][1] = 7
    grid.append([4, 5])
    print(grid, grid[0], len(grid[2]))
    print(grid == [[1, 7], [3], [4, 5]])
"#,
    ),
    (
        "list of floats and of bools",
        r#"fn main():
    xs = [1.5, 2.5]
    xs.append(3.5)
    print(xs, xs[0] + xs[1])
    flags = [True, False]
    flags.append(True)
    print(flags, flags[0], True in flags)
"#,
    ),
    (
        "list of strings",
        r#"fn main():
    names = ["a", "bb"]
    names.append("ccc")
    total = 0
    for name in names:
        total = total + len(name)
    print(names, total, names[1])
"#,
    ),
    (
        "tuple literal index and unpack",
        r#"fn main():
    t = (1, "a", True)
    print(t[0], t[1], t[2], t[-1])
    a, b, c = t
    print(a, b, c)
    x = 1
    y = 2
    x, y = y, x
    print(x, y)
    single = (7,)
    print(single, len(single))
"#,
    ),
    (
        "tuple as parameter and return value",
        r#"fn swap(t: tuple<int, str>) -> tuple<str, int>:
    return (t[1], t[0])

fn origin() -> tuple<float, float>:
    return (0.0, 0.0)

fn main():
    s, n = swap((1, "x"))
    print(s, n)
    print(origin())
    print(swap((2, "y")) == ("y", 2))
"#,
    ),
    (
        "tuple with a bool member",
        r#"fn main():
    t = (True, 1)
    flag, n = t
    print(flag, n, t)
    print(t == (True, 1))
    pairs = [(1, True), (2, False)]
    print(pairs[1], len(pairs))
"#,
    ),
    (
        "class field bool and tuple",
        r#"class Cell:
    at: tuple<int, int>
    alive: bool = False

fn main():
    c = Cell(at=(0, 0))
    c.alive = True
    c.at = (1, 2)
    print(c.alive, c.at, c.at[0])
"#,
    ),
    (
        "for over a list",
        r#"fn main():
    total = 0
    for n in [1, 2, 3]:
        if n == 2:
            continue
        total = total + n
    print(total)
    for row in [[1], [2, 3]]:
        print(len(row), row)
"#,
    ),
    (
        "for over a str",
        r#"fn main():
    count = 0
    for c in "héllo wörld":
        if c == " ":
            break
        count = count + 1
    print(count)
    for c in "ab":
        print(c, c.upper())
"#,
    ),
    (
        "every str method",
        r#"fn main():
    s = "  Hello World  "
    t = s.strip()
    print(t.upper(), t.lower())
    parts = t.split(" ")
    print(parts, "-".join(parts))
    print(t.startswith("Hello"), t.endswith("World"))
    print(t.find("World"), t.find("nope"))
    print(t.replace("World", "typhoon"))
    print(len(t), "lo W" in t, "z" not in t)
    print(t < "Z", t >= "A", t == "Hello World")
"#,
    ),
    (
        "str conversions",
        r#"fn main():
    print(str(1), str(-2))
    print(str(1.5), str(2.0))
    print(str(True), str(False))
    print(str("x"))
    print("n=" + str(3) + "!")
"#,
    ),
    (
        "printing every container shape",
        r#"fn main():
    print([1, 2])
    print([[1], [2, 3]])
    print(["a", "b"])
    print([1.5])
    print([True, False])
    print((1, "a", 2.5, True))
    print((1,))
    print([(1, "a")])
    print([""])
"#,
    ),
    (
        "empty containers",
        r#"fn main():
    xs: list<int> = []
    ys: list<str> = []
    grid: list<list<int>> = []
    print(len(xs), len(ys), len(grid))
    print(xs, ys, grid)
    grid.append(xs)
    print(grid)
"#,
    ),
    (
        "len of every shape",
        r#"fn main():
    xs = [1, 2, 3]
    s = "héllo"
    t = (1, 2)
    print(len(xs), len(s), len(t))
"#,
    ),
    (
        "class list and tuple together",
        r#"class Board:
    cells: list<int>
    size: tuple<int, int>
    label: str = "board"

    fn at(self, i: int) -> int:
        return self.cells[i]

    fn set(self, i: int, v: int):
        self.cells[i] = v

fn main():
    b = Board(cells=[0, 0, 0], size=(1, 3))
    b.set(1, 5)
    print(b.label, b.size, b.at(1), b.cells)
"#,
    ),
    (
        "f-strings over the new types",
        r#"fn main():
    xs = [1, 2]
    s = "hi"
    print(f"{len(xs)} {s} {s.upper()} {xs[0]:d} {float(xs[1]):.2f}")
"#,
    ),
];

#[test]
fn every_m2_construct_compiles_to_valid_llvm_ir() {
    let verifier = Verifier::find();
    let dir = std::env::temp_dir().join(format!("typhoon-m2-ir-check-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    assert!(
        M2_PROGRAMS.len() >= 20,
        "the M2 corpus is meant to be broad, found {}",
        M2_PROGRAMS.len()
    );

    let mut problems = Vec::new();
    for (index, (name, source)) in M2_PROGRAMS.iter().enumerate() {
        let text = match compile_to_llvm_ir("main.ty", source) {
            Ok(text) => text,
            Err(errors) => {
                problems.push(format!("`{name}` did not compile:\n{}", errors.join("\n")));
                continue;
            }
        };
        let path = dir.join(format!("m2-{index}")).with_extension("ll");
        std::fs::write(&path, &text).unwrap();
        if let Some(complaint) = verifier.check(&path) {
            problems.push(format!("`{name}` is not valid LLVM IR:\n{complaint}"));
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert!(problems.is_empty(), "{}", problems.join("\n\n"));
}

// ---------------------------------------------------------------------------
// The shapes DESIGN 2.2 promises are inline, and not runtime calls
// ---------------------------------------------------------------------------

/// Every function `ir` calls, deduplicated and sorted.
///
/// `declare` lines are ignored on purpose: what matters is what the emitted
/// code actually calls.
fn called_symbols(ir: &str) -> Vec<String> {
    let mut out: Vec<String> = ir
        .lines()
        .filter(|line| line.contains(" call "))
        .filter_map(|line| line.split_once('@'))
        .filter_map(|(_, rest)| rest.split_once('('))
        .map(|(name, _)| name.to_string())
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Asserts that nothing in `ir` calls a runtime helper whose name starts with
/// one of `prefixes`, except for the symbols in `allowed`.
#[track_caller]
fn no_runtime_calls(ir: &str, prefixes: &[&str], allowed: &[&str]) {
    for symbol in called_symbols(ir) {
        if prefixes.iter().any(|p| symbol.starts_with(p)) && !allowed.contains(&symbol.as_str()) {
            panic!("`{symbol}` should not be called here:\n{ir}");
        }
    }
}

#[test]
fn indexing_a_list_is_an_inline_bounds_check_and_a_load() {
    let text = ir("fn main():\n    xs = [1, 2, 3]\n    i = 1\n    print(xs[i])\n");
    // The bounds check is one unsigned comparison against the length, inline,
    // branching to an inline panic (DESIGN 4.3).
    assert!(
        text.contains("icmp ult i64") || text.contains("icmp uge i64"),
        "{text}"
    );
    assert!(text.contains("call void @ty_panic("), "{text}");
    // The element address is a `getelementptr` into the list's payload…
    assert!(
        text.contains("getelementptr inbounds %TyList, ptr"),
        "{text}"
    );
    assert!(text.contains("getelementptr inbounds i64, ptr"), "{text}");
    // …and the load itself calls nothing.
    no_runtime_calls(&text, &["ty_list_"], &["ty_list_new"]);
}

#[test]
fn appending_to_a_list_calls_the_runtime_only_when_it_is_full() {
    let text = ir("fn main():\n    xs: list<int> = []\n    xs.append(1)\n    print(len(xs))\n");
    // The capacity is read inline and compared with the length…
    assert!(
        text.contains("getelementptr inbounds %TyList, ptr"),
        "{text}"
    );
    assert!(text.contains("icmp sge i64"), "{text}");
    // …and `ty_list_grow` lives in the slow block, between the `append.grow`
    // label and the `append.store` one that both paths fall into.
    let grow_label = text
        .find("\nappend.grow")
        .unwrap_or_else(|| panic!("expected an `append.grow` block:\n{text}"));
    let grow_call = text
        .find("@ty_list_grow(")
        .unwrap_or_else(|| panic!("expected a call to `ty_list_grow`:\n{text}"));
    let store_label = text
        .find("\nappend.store")
        .unwrap_or_else(|| panic!("expected an `append.store` block:\n{text}"));
    assert!(grow_label < grow_call, "{text}");
    assert!(grow_call < store_label, "{text}");
    assert_eq!(
        text.matches("call void @ty_list_grow(").count(),
        1,
        "{text}"
    );
    // The store on the fast path is inline.
    assert!(text[store_label..].contains("store i64 1, ptr"), "{text}");
    no_runtime_calls(&text, &["ty_list_"], &["ty_list_new", "ty_list_grow"]);
}

#[test]
fn len_is_a_load_and_never_a_call() {
    let text = ir("fn main():\n    xs = [1, 2]\n    print(len(xs), len(\"héllo\"))\n");
    // `len(xs)` is the first field of the list header.
    assert!(text.contains("load i64, ptr"), "{text}");
    // `len(s)` is the code-point count in the string header, at offset 8, so
    // it is O(1) (DESIGN 4.3) rather than a scan.
    assert!(
        text.contains("getelementptr inbounds i8, ptr @str.0, i64 8"),
        "{text}"
    );
    no_runtime_calls(&text, &["ty_list_", "ty_str_"], &["ty_list_new"]);
}

#[test]
fn a_field_access_is_a_getelementptr_on_the_class_type() {
    let text = ir(
        "class Point:\n    x: float\n    y: float\n\nfn main():\n    p = Point(x=1.0, y=2.0)\n    p.y = p.x\n    print(p.y)\n",
    );
    // One named struct type per class, and every access is a `getelementptr`
    // into it — no accessor call.
    assert!(text.contains("%C.0 = type { double, double }"), "{text}");
    assert!(
        text.contains("getelementptr inbounds %C.0, ptr") && text.contains("i32 0, i32 1"),
        "{text}"
    );
    no_runtime_calls(&text, &["ty_field", "ty_class", "ty_get", "ty_set"], &[]);
}

#[test]
fn a_bool_is_an_i1_in_a_register_and_an_i8_in_a_container() {
    let text = ir(
        "class Flag:\n    on: bool\n\nfn main():\n    f = Flag(on=True)\n    b = f.on\n    flags = [b, False]\n    print(b, f.on, flags[0])\n",
    );
    // DESIGN 4.1/4.2: `bool` is `i1` in registers, `i8` in memory.
    assert!(text.contains("%C.0 = type { i8 }"), "{text}");
    assert!(text.contains("%b.addr = alloca i1, align 1"), "{text}");
    assert!(text.contains("zext i1"), "{text}");
    assert!(text.contains("trunc i8"), "{text}");
    assert!(text.contains("store i8"), "{text}");
    // A list of `bool` stores one byte per element.
    assert!(text.contains("@ty_list_new(i64 1, i8 1,"), "{text}");
}

#[test]
fn a_pointer_free_class_is_allocated_atomically() {
    // DESIGN 5.1: a block with no pointers in it is never scanned.
    let scalars = ir(
        "class Point:\n    x: float\n    n: int\n    on: bool\n\nfn main():\n    p = Point(x=1.0, n=1, on=True)\n    print(p.n)\n",
    );
    assert!(
        scalars.contains("call ptr @ty_alloc_atomic(i64 "),
        "{scalars}"
    );
    assert!(!scalars.contains("@ty_alloc(i64 "), "{scalars}");

    let pointers = ir(
        "class Row:\n    name: str\n    n: int\n\nfn main():\n    r = Row(name=\"a\", n=1)\n    print(r.n)\n",
    );
    assert!(pointers.contains("call ptr @ty_alloc(i64 "), "{pointers}");
    assert!(!pointers.contains("@ty_alloc_atomic("), "{pointers}");
}

#[test]
fn a_list_payload_is_atomic_only_when_the_elements_hold_no_pointers() {
    // `ty_list_new(elem_size, atomic, cap)`; the `i8` is the atomic flag.
    let ints = ir("fn main():\n    xs = [1, 2]\n    print(len(xs))\n");
    assert!(ints.contains("@ty_list_new(i64 8, i8 1,"), "{ints}");

    let strings = ir("fn main():\n    xs = [\"a\"]\n    print(len(xs))\n");
    assert!(strings.contains("@ty_list_new(i64 8, i8 0,"), "{strings}");

    let nested = ir("fn main():\n    xs = [[1]]\n    print(len(xs))\n");
    assert!(nested.contains("@ty_list_new(i64 8, i8 0,"), "{nested}");

    let pairs = ir("fn main():\n    xs = [(1, 2)]\n    print(len(xs))\n");
    assert!(pairs.contains("@ty_list_new(i64 16, i8 1,"), "{pairs}");
}

#[test]
fn a_tuple_is_a_first_class_aggregate() {
    let text = ir(
        "fn swap(t: tuple<int, str>) -> tuple<str, int>:\n    return (t[1], t[0])\n\nfn main():\n    a, b = swap((1, \"x\"))\n    print(a, b)\n",
    );
    // DESIGN 4.2: a tuple is a value, passed and returned by value.
    assert!(
        text.contains("define internal { ptr, i64 } @ty_user_swap({ i64, ptr } %t.arg)"),
        "{text}"
    );
    assert!(
        text.contains("insertvalue { ptr, i64 } poison, ptr"),
        "{text}"
    );
    assert!(text.contains("extractvalue { i64, ptr }"), "{text}");
    assert!(text.contains("ret { ptr, i64 }"), "{text}");
    // Nothing about a tuple is a runtime call or an allocation.
    no_runtime_calls(&text, &["ty_tuple", "ty_alloc"], &[]);
}

#[test]
fn a_string_literal_carries_its_byte_length_and_its_code_point_count() {
    let text = ir("fn main():\n    print(\"héllo\")\n");
    // `{ byte_len, char_len, bytes }`: 6 bytes, 5 code points.
    assert!(
        text.contains(
            "@str.0 = private unnamed_addr constant { i64, i64, [6 x i8] } \
             { i64 6, i64 5, [6 x i8] c\"h\\C3\\A9llo\" }"
        ),
        "{text}"
    );
    // Printing it reads the byte length from the header and the bytes from
    // just past it, with no allocation and no copy.
    assert!(
        text.contains("getelementptr inbounds i8, ptr @str.0, i64 16"),
        "{text}"
    );
}

#[test]
fn a_str_method_is_a_runtime_call_but_iteration_is_not() {
    // DESIGN 2.2: string algorithms allocate and are not hot, so they are
    // calls; walking a list is a loop the emitter writes itself.
    let text =
        ir("fn main():\n    s = \"a b\"\n    print(s.upper(), s.split(\" \"), s.find(\"b\"))\n");
    for symbol in ["ty_str_upper", "ty_str_split", "ty_str_find"] {
        assert!(text.contains(&format!("@{symbol}(")), "{text}");
    }

    let walk = ir(
        "fn main():\n    total = 0\n    for n in [1, 2]:\n        total = total + n\n    print(total)\n",
    );
    assert!(walk.contains("each.cond"), "{walk}");
    assert!(walk.contains("each.body"), "{walk}");
    no_runtime_calls(&walk, &["ty_list_", "ty_iter"], &["ty_list_new"]);
}
