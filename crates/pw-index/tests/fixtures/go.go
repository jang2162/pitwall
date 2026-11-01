// 심볼 인덱스 픽스처 — 바꾸면 스냅숏(tests/snapshots/)도 바뀐다.

package demo

type Point struct {
	X, Y int
}

type Shape interface {
	Area() float64
}

type ID int

func (p Point) Area() float64 {
	return 0
}

func NewPoint(x, y int) Point {
	return Point{X: x, Y: y}
}
