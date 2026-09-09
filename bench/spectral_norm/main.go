// spectral_norm: the Benchmarks Game eigenvalue benchmark. Ten power-method
// iterations of `AtA*u` over N-element vectors, where A is the implicit
// matrix A[i][j] = 1/((i+j)*(i+j+1)/2 + i + 1), followed by the Rayleigh
// quotient sqrt(vBv/vv).
//
// Measures `list<float>` code: sequential indexed reads of a float vector in
// a hot inner loop, one integer division and one float division per element,
// and a float accumulator kept in a register. Must stay structurally
// identical to main.ty (DESIGN 7.2): same three vectors, same loops, same
// order of operations.
//
// Expected output:
//
//	1.274224153
package main

import (
	"fmt"
	"math"
)

const (
	N          int = 6650
	Iterations int = 10
)

func evalA(i int, j int) float64 {
	return 1.0 / float64((i+j)*(i+j+1)/2+i+1)
}

// au = A * u
func evalATimesU(n int, u []float64, au []float64) {
	for i := 0; i < n; i++ {
		total := 0.0
		for j := 0; j < n; j++ {
			total = total + evalA(i, j)*u[j]
		}
		au[i] = total
	}
}

// au = At * u
func evalAtTimesU(n int, u []float64, au []float64) {
	for i := 0; i < n; i++ {
		total := 0.0
		for j := 0; j < n; j++ {
			total = total + evalA(j, i)*u[j]
		}
		au[i] = total
	}
}

// atAu = At * (A * u), using w as scratch
func evalAtATimesU(n int, u []float64, atAu []float64, w []float64) {
	evalATimesU(n, u, w)
	evalAtTimesU(n, w, atAu)
}

func main() {
	n := N
	u := []float64{}
	v := []float64{}
	w := []float64{}
	for i := 0; i < n; i++ {
		u = append(u, 1.0)
		v = append(v, 0.0)
		w = append(w, 0.0)
	}

	for i := 0; i < Iterations; i++ {
		evalAtATimesU(n, u, v, w)
		evalAtATimesU(n, v, u, w)
	}

	vBv := 0.0
	vv := 0.0
	for i := 0; i < n; i++ {
		vBv = vBv + u[i]*v[i]
		vv = vv + v[i]*v[i]
	}
	result := math.Sqrt(vBv / vv)
	fmt.Printf("%.9f\n", result)
}
