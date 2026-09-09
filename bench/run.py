#!/usr/bin/env python3
"""Run the typhoon benchmark suite against the Go baselines.

For every benchmark directory it builds `main.go` (always) and `main.ty`
(only if a typhoon compiler is available), runs both binaries `--runs` times,
takes the median wall-clock time and prints a Markdown table of the
typhoon/Go ratio against the targets in DESIGN section 2.

Standard library only, so it runs on the python3 that ships with macOS.
"""

import argparse
import json
import os
import platform
import shutil
import statistics
import subprocess
import sys
import time

BENCH_DIR = os.path.dirname(os.path.abspath(__file__))
REPO_ROOT = os.path.dirname(BENCH_DIR)

# name, target ratio (typhoon / go), the DESIGN section 2 row it comes from.
BENCHMARKS = [
    ("fib", 1.0, "pure CPU + recursion, M1"),
    ("loops", 1.0, "pure CPU + recursion, M1"),
    ("mandelbrot", 1.0, "float, M1"),
    ("nbody", 1.0, "float, M1"),
]

GO_FALLBACKS = ["/opt/homebrew/opt/go/bin/go", "/usr/local/go/bin/go"]
TYPHOON_FALLBACKS = [
    os.path.join(REPO_ROOT, "target", "release", "typhoon"),
    os.path.join(REPO_ROOT, "target", "debug", "typhoon"),
]


def find_go(explicit):
    if explicit:
        resolved = shutil.which(explicit)
        if resolved is None:
            die("--go %s is not an executable" % explicit)
        return resolved
    found = shutil.which("go")
    if found:
        return found
    for candidate in GO_FALLBACKS:
        if os.path.isfile(candidate) and os.access(candidate, os.X_OK):
            return candidate
    return None


def find_typhoon(explicit):
    if explicit:
        if not (os.path.isfile(explicit) and os.access(explicit, os.X_OK)):
            die("--typhoon %s is not an executable file" % explicit)
        return explicit
    for candidate in TYPHOON_FALLBACKS:
        if os.path.isfile(candidate) and os.access(candidate, os.X_OK):
            return candidate
    return None


def die(message):
    sys.stderr.write("error: %s\n" % message)
    sys.exit(2)


def build(cmd, cwd, what):
    proc = subprocess.run(cmd, cwd=cwd, capture_output=True)
    if proc.returncode != 0:
        sys.stderr.write("\n%s build failed in %s\n" % (what, cwd))
        sys.stderr.write("  $ %s\n" % " ".join(cmd))
        sys.stderr.write(proc.stdout.decode("utf-8", "replace"))
        sys.stderr.write(proc.stderr.decode("utf-8", "replace"))
        return False
    return True


def time_binary(binary, cwd, runs):
    """Warm up once, then time `runs` runs. Returns (times, stdout_bytes)."""
    warm = subprocess.run([binary], cwd=cwd, capture_output=True)
    if warm.returncode != 0:
        sys.stderr.write("\n%s exited with %d\n" % (binary, warm.returncode))
        sys.stderr.write(warm.stderr.decode("utf-8", "replace"))
        return None, None
    output = warm.stdout
    times = []
    for _ in range(runs):
        start = time.perf_counter()
        proc = subprocess.run([binary], cwd=cwd, capture_output=True)
        times.append(time.perf_counter() - start)
        if proc.returncode != 0:
            sys.stderr.write("\n%s exited with %d\n" % (binary, proc.returncode))
            return None, None
        if proc.stdout != output:
            sys.stderr.write("\n%s is not deterministic across runs\n" % binary)
            return None, None
    return times, output


def show(label, text):
    for line in text.decode("utf-8", "replace").splitlines() or [""]:
        sys.stdout.write("      %-8s | %s\n" % (label, line))


def main():
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--only", metavar="NAME", action="append",
                        help="run only this benchmark (repeatable)")
    parser.add_argument("--runs", type=int, default=5,
                        help="timed runs per binary, median is reported (default: 5)")
    parser.add_argument("--typhoon", metavar="PATH",
                        help="typhoon compiler to use (default: target/release/typhoon, "
                             "then target/debug/typhoon)")
    parser.add_argument("--go", metavar="PATH", help="go toolchain to use (default: $PATH)")
    parser.add_argument("--no-typhoon", action="store_true",
                        help="only build and time the Go baselines, even if a typhoon "
                             "compiler is available")
    parser.add_argument("--tolerance", type=float, default=0.0, metavar="FRACTION",
                        help="slack added to every target before deciding PASS/FAIL, e.g. "
                             "0.05 to allow 1.05x against a 1.0x target (default: 0.0). "
                             "Timing noise on a laptop is a few percent even for identical "
                             "binaries.")
    parser.add_argument("--json", metavar="FILE", help="dump the results as JSON")
    args = parser.parse_args()

    if args.runs < 1:
        die("--runs must be at least 1")

    selected = BENCHMARKS
    if args.only:
        known = set(entry[0] for entry in BENCHMARKS)
        for name in args.only:
            if name not in known:
                die("unknown benchmark %r (have: %s)"
                    % (name, ", ".join(entry[0] for entry in BENCHMARKS)))
        selected = [entry for entry in BENCHMARKS if entry[0] in args.only]

    go = find_go(args.go)
    if go is None:
        die("no go toolchain found; pass --go PATH")
    go_version = subprocess.run([go, "version"], capture_output=True).stdout.decode().strip()
    pinned = ""
    version_file = os.path.join(BENCH_DIR, "GO_VERSION")
    if os.path.isfile(version_file):
        pinned = open(version_file).read().strip()

    if args.no_typhoon:
        if args.typhoon:
            die("--typhoon and --no-typhoon are mutually exclusive")
        typhoon = None
    else:
        typhoon = find_typhoon(args.typhoon)

    print("machine  : %s %s" % (platform.machine(), platform.platform()))
    print("go       : %s (%s)" % (go_version, go))
    if pinned and pinned not in go_version.split():
        print("           NOTE: bench/GO_VERSION pins %s, this is a different toolchain" % pinned)
    if typhoon is None:
        if args.no_typhoon:
            print("typhoon  : skipped (--no-typhoon), building and timing the Go baselines only.")
        else:
            print("typhoon  : NOT FOUND -- building and timing the Go baselines only.")
            print("           Build the compiler (cargo build --release) or pass --typhoon PATH")
            print("           to get the comparison table.")
    else:
        print("typhoon  : %s" % typhoon)
    print("runs     : %d (median reported)" % args.runs)
    print("")

    results = []
    failures = []

    for name, target, note in selected:
        directory = os.path.join(BENCH_DIR, name)
        print("== %s" % name)
        row = {"name": name, "target": target, "note": note}

        if not build([go, "build", "-o", "go_bin", "./main.go"], directory, "go"):
            failures.append("%s: go build failed" % name)
            continue
        go_times, go_out = time_binary(os.path.join(directory, "go_bin"), directory, args.runs)
        if go_times is None:
            failures.append("%s: go binary failed" % name)
            continue
        row["go_median"] = statistics.median(go_times)
        row["go_times"] = go_times
        row["go_output"] = go_out.decode("utf-8", "replace")
        print("   go      : %.3fs" % row["go_median"])

        if typhoon is not None:
            if not build([typhoon, "build", "-O2", "main.ty", "-o", "ty_bin"], directory, "typhoon"):
                failures.append("%s: typhoon build failed" % name)
                results.append(row)
                continue
            ty_times, ty_out = time_binary(os.path.join(directory, "ty_bin"), directory, args.runs)
            if ty_times is None:
                failures.append("%s: typhoon binary failed" % name)
                results.append(row)
                continue
            row["ty_median"] = statistics.median(ty_times)
            row["ty_times"] = ty_times
            row["ty_output"] = ty_out.decode("utf-8", "replace")
            print("   typhoon : %.3fs" % row["ty_median"])

            row["output_match"] = go_out == ty_out
            if not row["output_match"]:
                print("")
                print("   !!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!")
                print("   !! OUTPUT MISMATCH in %s: the two implementations do not" % name)
                print("   !! agree, so the timing ratio below is meaningless.")
                print("   !!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!")
                show("go", go_out)
                show("typhoon", ty_out)
                print("")
                failures.append("%s: stdout mismatch between go and typhoon" % name)

            row["ratio"] = row["ty_median"] / row["go_median"] if row["go_median"] else float("inf")
            limit = target * (1.0 + args.tolerance)
            row["pass"] = row["output_match"] and row["ratio"] <= limit
            if not row["pass"] and row["output_match"]:
                failures.append("%s: ratio %.2fx exceeds target %.1fx (limit %.2fx)"
                                % (name, row["ratio"], target, limit))
        results.append(row)

    print("")
    header = "| benchmark | go (s) | typhoon (s) | ratio ty/go | target | result |"
    print(header)
    print("|---|---|---|---|---|---|")
    for row in results:
        if "ty_median" in row and "ratio" in row:
            print("| %s | %.3f | %.3f | %.2fx | <= %.1fx | %s |"
                  % (row["name"], row["go_median"], row["ty_median"], row["ratio"],
                     row["target"], "PASS" if row["pass"] else "FAIL"))
        else:
            what = "baseline only" if typhoon is None else "typhoon FAILED"
            print("| %s | %.3f | - | - | <= %.1fx | %s |"
                  % (row["name"], row["go_median"], row["target"], what))
    print("")
    print("Targets are the DESIGN section 2 exit criteria for M1 (typhoon <= 1.0x Go)%s."
          % (" plus a %.0f%% tolerance" % (args.tolerance * 100) if args.tolerance else ""))
    if typhoon is None:
        print("typhoon was not run, so nothing here can pass or fail yet.")

    if args.json:
        payload = {
            "machine": platform.machine(),
            "platform": platform.platform(),
            "go_version": go_version,
            "go_version_pinned": pinned,
            "typhoon": typhoon,
            "runs": args.runs,
            "tolerance": args.tolerance,
            "timestamp": time.strftime("%Y-%m-%dT%H:%M:%S%z"),
            "results": results,
        }
        with open(args.json, "w") as handle:
            json.dump(payload, handle, indent=2, sort_keys=True)
            handle.write("\n")
        print("wrote %s" % args.json)

    if failures:
        print("")
        for failure in failures:
            print("FAIL: %s" % failure)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
