// Module for the Go baseline implementations of the typhoon benchmarks.
// It exists only so `go vet ./...` works inside each benchmark directory;
// the benchmarks themselves use nothing outside the standard library.
module typhoon/bench

go 1.21
