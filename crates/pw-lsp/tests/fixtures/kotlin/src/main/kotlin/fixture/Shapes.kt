// 정의 · 구현 · rename 대상. 표지(`/*@이름*/`)는 그 이름의 첫 글자 바로 앞에 있다(픽스처 README 「앵커 표」).
package fixture

/** 넓이를 가진 도형. */
interface /*@def.shape*/Shape {
    /** 넓이. */
    fun /*@def.shape-area*/area(): Int
}

/** 사각형. */
class /*@impl.rect-shape*/Rect(val width: Int, val height: Int) : Shape {
    override fun area(): Int = width * height
}

/** 두 변으로 넓이를 낸다. rename 대상(이 정의와 Calls.kt 의 호출 셋). */
fun /*@rename.scale*/scale(width: Int, height: Int): Int = width * height
