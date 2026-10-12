// k_nucleotide: the Benchmarks Game hash-table benchmark. Count every k-mer
// of a DNA sequence in a map[string]int: the 1- and 2-mer frequency tables
// sorted by frequency, then the counts of five fixed sequences up to 18 long.
//
// The input is generated in-program rather than read from stdin: it is the
// THREE section (Homo sapiens frequencies) of the Benchmarks Game `fasta`
// output, upper-cased, produced by fasta's linear congruential generator
// after the 3 * N draws of section TWO. Measures string slicing and a
// string-keyed hash map (a zero-copy substring key for Go, one key
// allocation per k-mer for Typhoon). Must stay structurally identical to
// main.ty (DESIGN 7.2): same generator, same map traffic, same sort keys.
//
// Expected output:
//
//	A 30.298
//	T 30.147
//	C 19.802
//	G 19.754
//
//	AA 9.178
//	TA 9.131
//	AT 9.130
//	TT 9.089
//	CA 6.004
//	AC 6.002
//	AG 5.988
//	GA 5.984
//	CT 5.971
//	TC 5.970
//	GT 5.956
//	TG 5.956
//	CC 3.918
//	GC 3.912
//	CG 3.909
//	GG 3.901
//
//	88278	GGT
//	26769	GGTA
//	2833	GGTATT
//	53	GGTATTTTAATT
//	53	GGTATTTTAATTTATAGT
package main

import (
	"fmt"
	"sort"
	"strings"
)

// fasta's N: section THREE is 5 * N nucleotides long.
const N int = 1500000

const (
	IM  int     = 139968
	IA  int     = 3877
	IC  int     = 29573
	IMF float64 = 139968.0
)

// Lcg is fasta's random number generator: Next is random(1.0).
type Lcg struct {
	last int
}

func (r *Lcg) Next() float64 {
	r.last = (r.last*IA + IC) % IM
	return float64(r.last) / IMF
}

// pick returns the first letter whose cumulative probability exceeds r,
// else the last.
func pick(cumulative []float64, r float64) int {
	for i := 0; i < len(cumulative); i++ {
		if r < cumulative[i] {
			return i
		}
	}
	return len(cumulative) - 1
}

// makeSequence returns section THREE of `fasta n`, upper-cased.
func makeSequence(n int) string {
	rng := &Lcg{last: 42}
	// Section ONE repeats a fixed string and draws nothing; section TWO
	// draws one random number per nucleotide.
	for i := 0; i < 3*n; i++ {
		rng.Next()
	}
	probabilities := []float64{0.3029549426680, 0.1979883004921, 0.1975473066391, 0.3015094502008}
	letters := []string{"A", "C", "G", "T"}
	cumulative := []float64{}
	acc := 0.0
	for _, p := range probabilities {
		acc = acc + p
		cumulative = append(cumulative, acc)
	}
	parts := []string{}
	for i := 0; i < 5*n; i++ {
		parts = append(parts, letters[pick(cumulative, rng.Next())])
	}
	return strings.Join(parts, "")
}

// frequencies counts every k-mer of length frame.
func frequencies(seq string, frame int) map[string]int {
	counts := map[string]int{}
	n := len(seq) - frame + 1
	for i := 0; i < n; i++ {
		key := seq[i : i+frame]
		counts[key] = counts[key] + 1
	}
	return counts
}

// entry mirrors main.ty's (-count, key) tuples.
type entry struct {
	negCount int
	key      string
}

// writeFrequencies prints the k-mers of length frame by descending
// frequency, ties by name.
func writeFrequencies(seq string, frame int) {
	counts := frequencies(seq, frame)
	total := len(seq) - frame + 1
	order := []entry{}
	for key, count := range counts {
		order = append(order, entry{-count, key})
	}
	sort.SliceStable(order, func(i, j int) bool {
		if order[i].negCount != order[j].negCount {
			return order[i].negCount < order[j].negCount
		}
		return order[i].key < order[j].key
	})
	for _, e := range order {
		fmt.Printf("%s %.3f\n", e.key, 100.0*float64(-e.negCount)/float64(total))
	}
	fmt.Println("")
}

func writeCount(seq string, target string) {
	counts := frequencies(seq, len(target))
	fmt.Printf("%d\t%s\n", counts[target], target)
}

func main() {
	seq := makeSequence(N)
	writeFrequencies(seq, 1)
	writeFrequencies(seq, 2)
	for _, target := range []string{"GGT", "GGTA", "GGTATT", "GGTATTTTAATT", "GGTATTTTAATTTATAGT"} {
		writeCount(seq, target)
	}
}
