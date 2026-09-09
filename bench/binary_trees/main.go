// binary_trees: the Benchmarks Game allocation benchmark. Build perfectly
// balanced binary trees of two-pointer nodes and walk them counting nodes:
// one stretch tree, one long-lived tree, then a doubling loop that builds
// and immediately drops a great many short-lived trees.
//
// Measures the allocator and the GC rather than arithmetic: every node is a
// heap object with two nullable references, and the walk is one null test
// plus one add per node. Must stay structurally identical to main.ty
// (DESIGN 7.2): same node layout, same recursion, same tree counts.
//
// Expected output:
//
//	stretch tree of depth 19 check: 1048575
//	786432 trees of depth 4 check: 24379392
//	196608 trees of depth 6 check: 24969216
//	49152 trees of depth 8 check: 25116672
//	12288 trees of depth 10 check: 25153536
//	3072 trees of depth 12 check: 25162752
//	768 trees of depth 14 check: 25165056
//	192 trees of depth 16 check: 25165632
//	48 trees of depth 18 check: 25165776
//	long lived tree of depth 18 check: 524287
package main

import "fmt"

const (
	MinDepth int = 4
	MaxDepth int = 18
	// Trees built per depth, as a multiple of the classic
	// `1 << (MaxDepth-depth+MinDepth)`. See README.md: the classic count is a
	// pure power of two, so MaxDepth alone moves the running time in steps of
	// 2x (0.50 s / 1.00 s / 2.33 s here) and cannot be aimed at 1.5 s.
	Scale int = 3
)

type Node struct {
	left  *Node
	right *Node
}

func bottomUpTree(depth int) *Node {
	if depth > 0 {
		return &Node{left: bottomUpTree(depth - 1), right: bottomUpTree(depth - 1)}
	}
	return &Node{left: nil, right: nil}
}

func itemCheck(t *Node) int {
	left := t.left
	if left == nil {
		return 1
	}
	right := t.right
	if right == nil {
		return 1
	}
	return 1 + itemCheck(left) + itemCheck(right)
}

func main() {
	stretchDepth := MaxDepth + 1
	stretch := bottomUpTree(stretchDepth)
	fmt.Printf("stretch tree of depth %d check: %d\n", stretchDepth, itemCheck(stretch))

	longLivedTree := bottomUpTree(MaxDepth)

	for depth := MinDepth; depth <= MaxDepth; depth = depth + 2 {
		iterations := Scale << (MaxDepth - depth + MinDepth)
		check := 0
		for i := 0; i < iterations; i++ {
			check = check + itemCheck(bottomUpTree(depth))
		}
		fmt.Printf("%d trees of depth %d check: %d\n", iterations, depth, check)
	}

	fmt.Printf("long lived tree of depth %d check: %d\n", MaxDepth, itemCheck(longLivedTree))
}
