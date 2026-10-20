// A comment.
package main

import (
	"fmt"
	"strings"
)

type Point struct {
	X, Y int
}

func (p *Point) String() string {
	return fmt.Sprintf("%d,%d", p.X, p.Y)
}

func main() {
	p := &Point{X: 1, Y: 0x1F}
	if p != nil && true {
		fmt.Println(strings.ToUpper("ok\n"), p, 1.5)
	}
}
