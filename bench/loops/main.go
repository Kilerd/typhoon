// loops: nested integer loops with a constant modulus and a wrapping
// i64 accumulator.
//
// Measures integer loop codegen: induction variables, `%` by a constant,
// and keeping the accumulator in a register. No allocation, no floats.
// Must stay structurally identical to main.ty (DESIGN 7.2).
//
// Expected output:
//
//	9256937139
package main

import "fmt"

const (
	N int = 60000
	M int = 60000
)

func main() {
	acc := 0
	for i := 0; i < N; i++ {
		for j := 0; j < M; j++ {
			acc = acc + (i*j)%7
		}
	}
	fmt.Printf("%d\n", acc)
}
