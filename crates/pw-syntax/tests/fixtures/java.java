// A comment.
package dev.pitwall;

import java.util.List;

/** Doc comment. */
@Deprecated
public final class Point<T> implements Comparable<Point<T>> {
    private static final int LIMIT = 0x1F;
    private final T value;

    public Point(T value) {
        this.value = value;
    }

    @Override
    public int compareTo(Point<T> other) {
        return value == null ? -1 : "x\n".length() + LIMIT;
    }
}
