# binary_trees

The Benchmarks Game allocation benchmark: `bottom_up_tree(depth)` builds a perfectly
balanced tree of two-pointer nodes and `item_check(t)` walks it counting nodes. One
stretch tree of depth `MAX_DEPTH + 1`, one long-lived tree of depth `MAX_DEPTH` that
stays reachable until the last line, and in between a doubling loop that builds and
immediately drops a great many short-lived trees. Nearly all of the time is spent in
the allocator and the GC: the arithmetic is one `is None` test and one add per node.

- Parameters: `MIN_DEPTH = 4`, `MAX_DEPTH = 18`, `SCALE = 3`, i.e.
  `SCALE << (MAX_DEPTH - depth + MIN_DEPTH)` trees at each even depth from 4 to 18 —
  202 million nodes allocated in total, of which only the long-lived tree (524287
  nodes) and the stretch tree survive their loop iteration.
- `SCALE` exists because the classic tree count, `1 << (MAX_DEPTH - depth + MIN_DEPTH)`,
  is a pure power of two: with `SCALE = 1` this machine runs `MAX_DEPTH = 18` in
  0.50 s, 19 in 1.00 s and 20 in 2.33 s, so `MAX_DEPTH` alone cannot be aimed at the
  1.5 s the rest of the suite uses. `SCALE = 3` at `MAX_DEPTH = 18` simply builds three
  times as many trees at every depth; the allocation profile is unchanged. Set
  `SCALE = 1` and `MAX_DEPTH = 19` in both files to get the canonical output.
- Expected output (10 lines):

  ```
  stretch tree of depth 19 check: 1048575
  786432 trees of depth 4 check: 24379392
  196608 trees of depth 6 check: 24969216
  49152 trees of depth 8 check: 25116672
  12288 trees of depth 10 check: 25153536
  3072 trees of depth 12 check: 25162752
  768 trees of depth 14 check: 25165056
  192 trees of depth 16 check: 25165632
  48 trees of depth 18 check: 25165776
  long lived tree of depth 18 check: 524287
  ```

- Go baseline: **1.499 s** (median of 5, Apple M4, go1.27.1).
- The target is `<= 2.0x`, not `<= 1.0x`: DESIGN section 2.2 concedes the point for the
  Boehm phase (M2) and asks for `<= 1.0x` only after escape analysis lands (M5). Every
  node here escapes into a tree, so Go cannot stack-allocate them either; what Go has
  and typhoon does not is a per-P bump allocator and a precise concurrent collector,
  against Boehm's general-purpose allocator and conservative stop-the-world mark-sweep.
- `Node.left` / `Node.right` are `Node | None` — a nullable class reference narrowed by
  `is None`, matching Go's `*Node` and `nil`. `item_check` returns early on a nil child
  instead of testing both, exactly as the Go version does, so the two do the same number
  of loads and branches per node.
