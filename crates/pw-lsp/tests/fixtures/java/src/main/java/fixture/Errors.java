// 오류 자리(진단). 이 파일만 일부러 컴파일되지 않는다 — 진단 수는 이 파일 안으로만 단언한다.
package fixture;

/** 타입이 맞지 않는다. */
final class Errors {
    private Errors() {}

    static int mismatch() {
        int n = /*@error.type-mismatch*/"text";
        return n;
    }
}
