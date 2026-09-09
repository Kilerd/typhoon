// fannkuch: fannkuch-redux over the permutations of [0, N).
//
// Measures `list<int>` code: indexing, in-place element swaps and three
// tight loops over small arrays, with no allocation inside the permutation
// loop (the three lists are grown once, up front) and no floats. Must stay
// structurally identical to main.ty (DESIGN 7.2): same lists, same loops,
// same order of permutations.
//
// Expected output:
//
//	556355
//	Pfannkuchen(11) = 51
package main

import "fmt"

const N int = 11

// fannkuch returns (checksum, maxFlips) for the permutations of [0, n).
func fannkuch(n int) (int, int) {
	perm := []int{}
	perm1 := []int{}
	count := []int{}
	for i := 0; i < n; i++ {
		perm = append(perm, 0)
		perm1 = append(perm1, i)
		count = append(count, 0)
	}

	checksum := 0
	maxFlips := 0
	permCount := 0
	r := n
	for {
		// Rotate the first r elements back into their reset state.
		for r != 1 {
			count[r-1] = r
			r = r - 1
		}

		for i := 0; i < n; i++ {
			perm[i] = perm1[i]
		}

		// Flip: reverse the first perm[0]+1 elements until perm[0] is 0.
		flips := 0
		k := perm[0]
		for k != 0 {
			i := 0
			j := k
			for i < j {
				t := perm[i]
				perm[i] = perm[j]
				perm[j] = t
				i = i + 1
				j = j - 1
			}
			flips = flips + 1
			k = perm[0]
		}

		if flips > maxFlips {
			maxFlips = flips
		}
		if permCount%2 == 0 {
			checksum = checksum + flips
		} else {
			checksum = checksum - flips
		}
		permCount = permCount + 1

		// Next permutation, in the order the benchmark prescribes.
		done := false
		for {
			if r == n {
				done = true
				break
			}
			p0 := perm1[0]
			i := 0
			for i < r {
				perm1[i] = perm1[i+1]
				i = i + 1
			}
			perm1[r] = p0
			count[r] = count[r] - 1
			if count[r] > 0 {
				break
			}
			r = r + 1
		}
		if done {
			break
		}
	}
	return checksum, maxFlips
}

func main() {
	checksum, maxFlips := fannkuch(N)
	fmt.Printf("%d\n", checksum)
	fmt.Printf("Pfannkuchen(%d) = %d\n", N, maxFlips)
}
