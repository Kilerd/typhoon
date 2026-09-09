# fannkuch

fannkuch-redux over the permutations of `[0, N)`. For each permutation, reverse the
first `perm[0] + 1` elements until `perm[0]` is 0 and count the reversals ("flips");
report the largest flip count and a checksum that adds the flips of even-numbered
permutations and subtracts the odd ones. Three `list<int>` of length `N` are grown once
before the permutation loop; the loop itself only indexes, swaps and copies, so this
measures `list<int>` element access and small tight loops, not allocation.

- Parameters: `N = 11`, i.e. 39,916,800 permutations, 278 million flips, 784 million
  element swaps and 439 million elements copied. `N = 12` is the next available size
  and is 12x the work (~18 s in Go), which is why 11 is used.
- Expected output (2 lines):

  ```
  556355
  Pfannkuchen(11) = 51
  ```

  Both are the reference values published with the benchmark, so a wrong answer is
  obvious. (`N = 10` gives `73196` / `38`, `N = 7` gives `228` / `16` if you want a
  quick check while working on the compiler.)
- Go baseline: **1.480 s** (median of 5, Apple M4, go1.27.1).
- The Go version builds its three slices with `append` on an empty slice rather than
  `make`, because that is what the M2 typhoon subset can express (`xs: list<int> = []`
  then `xs.append(...)`); both therefore perform the same growth sequence before the
  timed work and the same indexed accesses after it.
- The element swap uses an explicit temporary in both languages (no `a, b = b, a` on
  subscripts), and `fannkuch` returns a `tuple<int, int>` on the typhoon side against
  Go's two return values — both are unboxed pairs, and it happens once per run.
- `perm_count % 2` is always applied to a non-negative value, so typhoon's floor `%` and
  Go's truncating `%` agree.
