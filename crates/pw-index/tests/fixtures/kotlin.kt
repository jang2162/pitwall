// 심볼 인덱스 픽스처 — 바꾸면 스냅숏(tests/snapshots/)도 바뀐다.

package demo

const val LIMIT = 8

interface Shape {
    fun area(): Double
}

enum class Color {
    RED,
    GREEN,
}

data class Point(val x: Int, val y: Int)

class Circle(private val radius: Double) : Shape {
    val name = "circle"

    override fun area(): Double = 3.14 * radius * radius

    companion object Factory {
        fun unit(): Circle = Circle(1.0)
    }
}

object Registry {
    fun register(shape: Shape) {}
}

typealias Shapes = List<Shape>

fun makeCircle(): Circle {
    val radius = 2.0
    return Circle(radius)
}
