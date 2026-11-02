// M4-08 내비게이션 live 픽스처. 표지 주석(`/*@이름*/`)과 그 뜻은 ../../README.md 「앵커 표」.

/** 정의로 이동 · 사용처의 대상. */
export function /*@nav.def*/greet(name: string): number {
  return name.length;
}

/** 호출 둘 — 사용처(선언 포함)는 이 파일 안에 셋. */
export function twice(): number {
  return /*@nav.call-1*/greet("a") + /*@nav.call-2*/greet("b");
}
