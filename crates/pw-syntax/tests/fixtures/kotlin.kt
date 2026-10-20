// A comment.
package dev.pitwall

import kotlin.math.abs

/** Doc comment. */
data class Point(val x: Int, var y: Double = 1.5)

fun Point.norm(): Double {
    val label = "point ${x}\n"
    return if (x < 0 && true) abs(y) else y * 0x1F
}

object Registry {
    private val items = mutableListOf<Point>()
    fun add(p: Point) = items.add(p)
}
