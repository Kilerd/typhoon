# k_nucleotide

The Benchmarks Game hash-table benchmark. Every k-mer (substring of length `k`) of a
DNA sequence is counted in a string-keyed hash table: the 1- and 2-mer tables are
printed as percentages sorted by descending frequency (ties by name), then the counts
of `GGT`, `GGTA`, `GGTATT`, `GGTATTTTAATT` and `GGTATTTTAATTTATAGT` — seven counting
passes over the whole sequence, one dict operation pair per position.

- Input: generated in-program instead of read from stdin, so the runner needs no input
  file. It is section THREE (the Homo sapiens frequencies) of the Benchmarks Game
  `fasta` output, upper-cased: fasta's linear congruential generator
  (`IM = 139968`, `IA = 3877`, `IC = 29573`, seed 42) is first advanced by the
  `3 * N` draws of section TWO, then drives `5 * N` cumulative-probability picks.
  Generation is part of the timed run in both languages and is written the same way
  (a list of one-letter strings, then one join).
- Parameters: `N = 1500000`, i.e. a 7.5 M nucleotide sequence and 52.5 M k-mer lookups.
  The generator's period is 139968, so the sequence repeats itself every 139968
  letters: the long k-mers (12, 18) have about 140 k distinct keys, the short ones
  4 and 16.
- Expected output (27 lines):

  ```
  A 30.298
  T 30.147
  C 19.802
  G 19.754

  AA 9.178
  TA 9.131
  AT 9.130
  TT 9.089
  CA 6.004
  AC 6.002
  AG 5.988
  GA 5.984
  CT 5.971
  TC 5.970
  GT 5.956
  TG 5.956
  CC 3.918
  GC 3.912
  CG 3.909
  GG 3.901

  88278	GGT
  26769	GGTA
  2833	GGTATT
  53	GGTATTTTAATT
  53	GGTATTTTAATTTATAGT
  ```

  With `N = 25000` (the Benchmarks Game's test size) the Typhoon, Go and a plain
  Python version of the same program print identical output.
- Go baseline: **1.08-1.36 s** (medians of 3-5 runs, Apple M4, go1.27.1; measured while
  other builds were running, load average 6-12, so treat the spread as noise).
- Target: `<= 1.5x` Go at M3 (DESIGN section 2), `<= 1.0x` at M4.

## What is being compared

Both sides do the same work per position — `key = seq[i:i + k]`, then
`counts[key] = counts.get(key, 0) + 1` (Go: `counts[key] = counts[key] + 1`) — on
their builtin string-keyed table, and both sort the small tables by a
`(-count, key)` order. The differences are in the data structures, not the program:

- **Slicing.** A Go substring shares the sequence's bytes; a Typhoon `str` carries its
  `{byte_len, char_len}` header in front of its bytes, so `seq[i:i + k]` allocates a
  `16 + k` byte block (Boehm `GC_malloc_atomic`). A one-letter ASCII slice comes from
  the runtime's static table instead, so the `k = 1` pass allocates nothing.
- **The table.** Typhoon's `dict` is an insertion-ordered compact table
  (`crates/runtime/src/dict.rs`): an append-only entry array `{hash, key, value}` plus
  a linear-probing index whose slots carry 32 bits of the hash, so a probe touches an
  entry only on a likely match. Go's map is a Swiss table.
- **The double lookup.** `counts.get(key, 0)` and the following `counts[key] = ...`
  hash and probe once: the table remembers its last probe (key word, hash, slot), and a
  `str` key word is the string's address, which is the same object both times.

## Results

Three separate runs of `./run.sh` (3 or 5 timed runs each) gave **0.91x, 0.99x and
1.07x** (go 1.956 s / typhoon 1.771 s, go 1.182 s / typhoon 1.167 s, go 1.076 s /
typhoon 1.148 s):
comfortably inside the `<= 1.5x` M3 target, and around parity. Every other benchmark
emits byte-identical LLVM IR before and after dict/set landed. A `sample` profile of
the Typhoon binary puts about half of its time in the probe itself (cache misses on
the index slot, the entry and the key bytes — the same misses Go's map takes) and
about a fifth in slicing plus the key allocation, which is where the M4 string work
(a substring that shares its parent's bytes) would cut.
