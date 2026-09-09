#!/usr/bin/env bash
#
# Builds the playground: compiles crates/playground to WebAssembly, runs
# wasm-bindgen over it into web/pkg, and copies examples/ into web/examples
# together with the index the page fetches.
#
# Runs from any working directory. Preview the result with:
#
#     web/build.sh && python3 -m http.server -d web 8000
#     # then open http://localhost:8000/

set -euo pipefail

web_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root_dir="$(cd "$web_dir/.." && pwd)"
target="wasm32-unknown-unknown"
# A profile of our own: the workspace's [profile.release] carries debug info
# for the native compiler, which would triple the size of the .wasm.
profile="wasm-release"

# The wasm-bindgen CLI must be the exact version of the wasm-bindgen crate the
# module was built against, so take the version from the lockfile.
wb_version="$(
  awk '/^name = "wasm-bindgen"$/ { found = 1; next }
       found && /^version = / { gsub(/[",]/, "", $3); print $3; exit }' \
    "$root_dir/Cargo.lock"
)"
if [ -z "$wb_version" ]; then
  echo "error: could not find the wasm-bindgen version in Cargo.lock" >&2
  exit 1
fi

if ! command -v wasm-bindgen >/dev/null 2>&1; then
  echo "error: wasm-bindgen not found; install it with" >&2
  echo "    cargo install wasm-bindgen-cli --version $wb_version" >&2
  exit 1
fi

installed="$(wasm-bindgen --version | awk '{ print $2 }')"
if [ "$installed" != "$wb_version" ]; then
  echo "error: wasm-bindgen $installed does not match the crate ($wb_version); install it with" >&2
  echo "    cargo install wasm-bindgen-cli --version $wb_version --force" >&2
  exit 1
fi

if ! rustc --print target-list | grep -qx "$target"; then
  echo "error: this toolchain does not know the target $target" >&2
  exit 1
fi

echo "==> cargo build -p typhoon-playground --target $target --profile $profile"
cargo build \
  --manifest-path "$root_dir/Cargo.toml" \
  -p typhoon-playground \
  --target "$target" \
  --profile "$profile"

wasm_in="$root_dir/target/$target/$profile/typhoon_playground.wasm"
if [ ! -f "$wasm_in" ]; then
  echo "error: $wasm_in was not produced" >&2
  exit 1
fi

echo "==> wasm-bindgen --target web --out-dir web/pkg"
rm -rf "$web_dir/pkg"
wasm-bindgen "$wasm_in" \
  --target web \
  --no-typescript \
  --out-dir "$web_dir/pkg"

# ---------------------------------------------------------------- examples
#
# The dropdown fetches web/examples/index.json; every entry is one
# examples/*.ty file with the milestone it declares (DESIGN 7.1) and its first
# comment line as a description.
echo "==> copying examples"
rm -rf "$web_dir/examples"
mkdir -p "$web_dir/examples"

index="$web_dir/examples/index.json"
printf '[\n' >"$index"
first=1
for source in "$root_dir/examples/"*.ty; do
  [ -e "$source" ] || continue
  file="$(basename "$source")"
  # The golden-test directives (`# expect:`, `# milestone:`, ...) mean nothing
  # in the browser and read like syntax, so the copy keeps only the prose.
  # `sed -E` for portability: BSD sed (macOS) has no \| alternation.
  sed -E '/^# *(milestone|expect|error|exit|stderr):/d' "$source" \
    >"$web_dir/examples/$file"

  milestone="$(sed -n 's/^# milestone: *\([A-Za-z0-9]*\).*/\1/p' "$source" | head -n 1)"
  [ -n "$milestone" ] || milestone="?"
  # The first comment line that is prose rather than a test directive
  # describes what the example shows.
  description="$(
    awk '/^#/ {
           line = $0
           sub(/^# ?/, "", line)
           if (line !~ /^(milestone|expect|error|exit|stderr):/ && line != "") {
             print line
             exit
           }
         }' "$source" | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g'
  )"

  [ "$first" -eq 1 ] || printf ',\n' >>"$index"
  first=0
  printf '  { "name": "%s", "file": "%s", "milestone": "%s", "description": "%s" }' \
    "${file%.ty}" "$file" "$milestone" "$description" >>"$index"
done
printf '\n]\n' >>"$index"

echo
echo "built:"
for artifact in "$web_dir/pkg/typhoon_playground.js" "$web_dir/pkg/typhoon_playground_bg.wasm"; do
  printf '    %-28s %s\n' "web/pkg/$(basename "$artifact")" "$(du -h "$artifact" | cut -f1)"
done
echo "    $(grep -c '"file"' "$index") examples in web/examples/"
echo
echo "preview with: python3 -m http.server -d web 8000"
