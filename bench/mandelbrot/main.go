// mandelbrot: escape-time iteration over a W x H grid, counting the points
// that never escape within MaxIter iterations.
//
// Measures scalar float64 loop codegen: no allocation, no calls, a short
// data-dependent inner loop with an early exit.
// Must stay structurally identical to main.ty (DESIGN 7.2).
//
// Every multiply that feeds an add is broken out into its own local so that
// neither Go (which fuses into FMA on arm64) nor LLVM can contract a
// mul+add pair. Without this the two implementations would round
// differently and, this close to the boundary of the set, would disagree on
// the final count.
//
// Expected output:
//
//	21467454
package main

import "fmt"

const (
	W       int = 9000
	H       int = 9000
	MaxIter int = 50
)

func main() {
	count := 0
	for py := 0; py < H; py++ {
		v := float64(py) / float64(H) * 2.0
		cy := v - 1.0
		for px := 0; px < W; px++ {
			u := float64(px) / float64(W) * 3.0
			cx := u - 2.0
			x := 0.0
			y := 0.0
			escaped := false
			for k := 0; k < MaxIter; k++ {
				x2 := x * x
				y2 := y * y
				if x2+y2 > 4.0 {
					escaped = true
					break
				}
				t := 2.0 * x * y
				y = t + cy
				x = x2 - y2 + cx
			}
			if !escaped {
				count = count + 1
			}
		}
	}
	fmt.Printf("%d\n", count)
}
