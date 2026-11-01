// 오류 자리(진단). 이 파일만 일부러 타입 검사를 지나지 못한다 — 진단 수는 이 파일 안으로만 단언한다.

/** 타입이 맞지 않는다(TS2322). */
export function mismatch(): number {
  const /*@error.type-mismatch*/n: number = "text";
  return n;
}

/** 없는 이름(TS2304). */
export function unresolved(): number {
  return /*@error.unresolved*/missingFunction();
}
