// nbody: the Benchmarks Game 5-body integrator (sun + 4 gas giants).
//
// M1 typhoon has no lists and no classes, so the whole system lives in 35
// scalar locals (5 bodies x x/y/z/vx/vy/vz/mass) and the 10 pairwise
// interactions are written out by hand. This Go version keeps exactly the
// same shape so the comparison is fair (DESIGN 7.2): same 35 locals, same
// unrolling, same order of operations.
//
// Expected output:
//
//	-0.169075164
//	-0.169059907
package main

import (
	"fmt"
	"math"
)

const N int = 50000000

func energy(
	x0, y0, z0, vx0, vy0, vz0, m0 float64,
	x1, y1, z1, vx1, vy1, vz1, m1 float64,
	x2, y2, z2, vx2, vy2, vz2, m2 float64,
	x3, y3, z3, vx3, vy3, vz3, m3 float64,
	x4, y4, z4, vx4, vy4, vz4, m4 float64,
) float64 {
	var dx, dy, dz float64
	e := 0.0
	e = e + 0.5*m0*(vx0*vx0+vy0*vy0+vz0*vz0)
	dx = x0 - x1
	dy = y0 - y1
	dz = z0 - z1
	e = e - m0*m1/math.Sqrt(dx*dx+dy*dy+dz*dz)
	dx = x0 - x2
	dy = y0 - y2
	dz = z0 - z2
	e = e - m0*m2/math.Sqrt(dx*dx+dy*dy+dz*dz)
	dx = x0 - x3
	dy = y0 - y3
	dz = z0 - z3
	e = e - m0*m3/math.Sqrt(dx*dx+dy*dy+dz*dz)
	dx = x0 - x4
	dy = y0 - y4
	dz = z0 - z4
	e = e - m0*m4/math.Sqrt(dx*dx+dy*dy+dz*dz)
	e = e + 0.5*m1*(vx1*vx1+vy1*vy1+vz1*vz1)
	dx = x1 - x2
	dy = y1 - y2
	dz = z1 - z2
	e = e - m1*m2/math.Sqrt(dx*dx+dy*dy+dz*dz)
	dx = x1 - x3
	dy = y1 - y3
	dz = z1 - z3
	e = e - m1*m3/math.Sqrt(dx*dx+dy*dy+dz*dz)
	dx = x1 - x4
	dy = y1 - y4
	dz = z1 - z4
	e = e - m1*m4/math.Sqrt(dx*dx+dy*dy+dz*dz)
	e = e + 0.5*m2*(vx2*vx2+vy2*vy2+vz2*vz2)
	dx = x2 - x3
	dy = y2 - y3
	dz = z2 - z3
	e = e - m2*m3/math.Sqrt(dx*dx+dy*dy+dz*dz)
	dx = x2 - x4
	dy = y2 - y4
	dz = z2 - z4
	e = e - m2*m4/math.Sqrt(dx*dx+dy*dy+dz*dz)
	e = e + 0.5*m3*(vx3*vx3+vy3*vy3+vz3*vz3)
	dx = x3 - x4
	dy = y3 - y4
	dz = z3 - z4
	e = e - m3*m4/math.Sqrt(dx*dx+dy*dy+dz*dz)
	e = e + 0.5*m4*(vx4*vx4+vy4*vy4+vz4*vz4)
	return e
}

func main() {
	pi := 3.141592653589793
	solarMass := 4.0 * pi * pi
	daysPerYear := 365.24
	dt := 0.01

	// sun
	x0 := 0.0
	y0 := 0.0
	z0 := 0.0
	vx0 := 0.0
	vy0 := 0.0
	vz0 := 0.0
	m0 := solarMass

	// jupiter
	x1 := 4.84143144246472090
	y1 := -1.16032004402742839
	z1 := -0.103622044471123109
	vx1 := 0.00166007664274403694 * daysPerYear
	vy1 := 0.00769901118419740425 * daysPerYear
	vz1 := -0.0000690460016972063023 * daysPerYear
	m1 := 0.000954791938424326609 * solarMass

	// saturn
	x2 := 8.34336671824457987
	y2 := 4.12479856412430479
	z2 := -0.403523417114321381
	vx2 := -0.00276742510726862411 * daysPerYear
	vy2 := 0.00499852801234917238 * daysPerYear
	vz2 := 0.0000230417297573763929 * daysPerYear
	m2 := 0.000285885980666130812 * solarMass

	// uranus
	x3 := 12.8943695621391310
	y3 := -15.1111514016986312
	z3 := -0.223307578892655734
	vx3 := 0.00296460137564761618 * daysPerYear
	vy3 := 0.00237847173959480950 * daysPerYear
	vz3 := -0.0000296589568540237556 * daysPerYear
	m3 := 0.0000436624404335156298 * solarMass

	// neptune
	x4 := 15.3796971148509165
	y4 := -25.9193146099879641
	z4 := 0.179258772950371181
	vx4 := 0.00268067772490389322 * daysPerYear
	vy4 := 0.00162824170038242295 * daysPerYear
	vz4 := -0.0000951592254519715870 * daysPerYear
	m4 := 0.0000515138902046611451 * solarMass

	// offset momentum so the whole system stays put
	px := vx0*m0 + vx1*m1 + vx2*m2 + vx3*m3 + vx4*m4
	py := vy0*m0 + vy1*m1 + vy2*m2 + vy3*m3 + vy4*m4
	pz := vz0*m0 + vz1*m1 + vz2*m2 + vz3*m3 + vz4*m4
	vx0 = -px / solarMass
	vy0 = -py / solarMass
	vz0 = -pz / solarMass

	eBefore := energy(
		x0, y0, z0, vx0, vy0, vz0, m0,
		x1, y1, z1, vx1, vy1, vz1, m1,
		x2, y2, z2, vx2, vy2, vz2, m2,
		x3, y3, z3, vx3, vy3, vz3, m3,
		x4, y4, z4, vx4, vy4, vz4, m4,
	)
	fmt.Printf("%.9f\n", eBefore)

	var dx, dy, dz, d2, mag float64
	for step := 0; step < N; step++ {
		dx = x0 - x1
		dy = y0 - y1
		dz = z0 - z1
		d2 = dx*dx + dy*dy + dz*dz
		mag = dt / (d2 * math.Sqrt(d2))
		vx0 = vx0 - dx*m1*mag
		vy0 = vy0 - dy*m1*mag
		vz0 = vz0 - dz*m1*mag
		vx1 = vx1 + dx*m0*mag
		vy1 = vy1 + dy*m0*mag
		vz1 = vz1 + dz*m0*mag
		dx = x0 - x2
		dy = y0 - y2
		dz = z0 - z2
		d2 = dx*dx + dy*dy + dz*dz
		mag = dt / (d2 * math.Sqrt(d2))
		vx0 = vx0 - dx*m2*mag
		vy0 = vy0 - dy*m2*mag
		vz0 = vz0 - dz*m2*mag
		vx2 = vx2 + dx*m0*mag
		vy2 = vy2 + dy*m0*mag
		vz2 = vz2 + dz*m0*mag
		dx = x0 - x3
		dy = y0 - y3
		dz = z0 - z3
		d2 = dx*dx + dy*dy + dz*dz
		mag = dt / (d2 * math.Sqrt(d2))
		vx0 = vx0 - dx*m3*mag
		vy0 = vy0 - dy*m3*mag
		vz0 = vz0 - dz*m3*mag
		vx3 = vx3 + dx*m0*mag
		vy3 = vy3 + dy*m0*mag
		vz3 = vz3 + dz*m0*mag
		dx = x0 - x4
		dy = y0 - y4
		dz = z0 - z4
		d2 = dx*dx + dy*dy + dz*dz
		mag = dt / (d2 * math.Sqrt(d2))
		vx0 = vx0 - dx*m4*mag
		vy0 = vy0 - dy*m4*mag
		vz0 = vz0 - dz*m4*mag
		vx4 = vx4 + dx*m0*mag
		vy4 = vy4 + dy*m0*mag
		vz4 = vz4 + dz*m0*mag
		dx = x1 - x2
		dy = y1 - y2
		dz = z1 - z2
		d2 = dx*dx + dy*dy + dz*dz
		mag = dt / (d2 * math.Sqrt(d2))
		vx1 = vx1 - dx*m2*mag
		vy1 = vy1 - dy*m2*mag
		vz1 = vz1 - dz*m2*mag
		vx2 = vx2 + dx*m1*mag
		vy2 = vy2 + dy*m1*mag
		vz2 = vz2 + dz*m1*mag
		dx = x1 - x3
		dy = y1 - y3
		dz = z1 - z3
		d2 = dx*dx + dy*dy + dz*dz
		mag = dt / (d2 * math.Sqrt(d2))
		vx1 = vx1 - dx*m3*mag
		vy1 = vy1 - dy*m3*mag
		vz1 = vz1 - dz*m3*mag
		vx3 = vx3 + dx*m1*mag
		vy3 = vy3 + dy*m1*mag
		vz3 = vz3 + dz*m1*mag
		dx = x1 - x4
		dy = y1 - y4
		dz = z1 - z4
		d2 = dx*dx + dy*dy + dz*dz
		mag = dt / (d2 * math.Sqrt(d2))
		vx1 = vx1 - dx*m4*mag
		vy1 = vy1 - dy*m4*mag
		vz1 = vz1 - dz*m4*mag
		vx4 = vx4 + dx*m1*mag
		vy4 = vy4 + dy*m1*mag
		vz4 = vz4 + dz*m1*mag
		dx = x2 - x3
		dy = y2 - y3
		dz = z2 - z3
		d2 = dx*dx + dy*dy + dz*dz
		mag = dt / (d2 * math.Sqrt(d2))
		vx2 = vx2 - dx*m3*mag
		vy2 = vy2 - dy*m3*mag
		vz2 = vz2 - dz*m3*mag
		vx3 = vx3 + dx*m2*mag
		vy3 = vy3 + dy*m2*mag
		vz3 = vz3 + dz*m2*mag
		dx = x2 - x4
		dy = y2 - y4
		dz = z2 - z4
		d2 = dx*dx + dy*dy + dz*dz
		mag = dt / (d2 * math.Sqrt(d2))
		vx2 = vx2 - dx*m4*mag
		vy2 = vy2 - dy*m4*mag
		vz2 = vz2 - dz*m4*mag
		vx4 = vx4 + dx*m2*mag
		vy4 = vy4 + dy*m2*mag
		vz4 = vz4 + dz*m2*mag
		dx = x3 - x4
		dy = y3 - y4
		dz = z3 - z4
		d2 = dx*dx + dy*dy + dz*dz
		mag = dt / (d2 * math.Sqrt(d2))
		vx3 = vx3 - dx*m4*mag
		vy3 = vy3 - dy*m4*mag
		vz3 = vz3 - dz*m4*mag
		vx4 = vx4 + dx*m3*mag
		vy4 = vy4 + dy*m3*mag
		vz4 = vz4 + dz*m3*mag
		x0 = x0 + dt*vx0
		y0 = y0 + dt*vy0
		z0 = z0 + dt*vz0
		x1 = x1 + dt*vx1
		y1 = y1 + dt*vy1
		z1 = z1 + dt*vz1
		x2 = x2 + dt*vx2
		y2 = y2 + dt*vy2
		z2 = z2 + dt*vz2
		x3 = x3 + dt*vx3
		y3 = y3 + dt*vy3
		z3 = z3 + dt*vz3
		x4 = x4 + dt*vx4
		y4 = y4 + dt*vy4
		z4 = z4 + dt*vz4
	}

	eAfter := energy(
		x0, y0, z0, vx0, vy0, vz0, m0,
		x1, y1, z1, vx1, vy1, vz1, m1,
		x2, y2, z2, vx2, vy2, vz2, m2,
		x3, y3, z3, vx3, vy3, vz3, m3,
		x4, y4, z4, vx4, vy4, vz4, m4,
	)
	fmt.Printf("%.9f\n", eAfter)
}
