// fib: naive recursive Fibonacci.
//
// Measures pure function-call / recursion overhead: no allocation, no floats,
// no loops. Must stay structurally identical to main.ty (DESIGN 7.2).
//
// Expected output:
//
//	701408733
package main

import "fmt"

const N int = 44

func fib(n int) int {
	if n < 2 {
		return n
	}
	return fib(n-1) + fib(n-2)
}

func main() {
	fmt.Printf("%d\n", fib(N))
}
