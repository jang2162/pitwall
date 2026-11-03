// M4-16 작업 공간 심볼 live 픽스처 — 서버가 이름 자리가 아니라 선언의 처음을 주는 흔한 꼴(데코레이터가 붙은 클래스 · 이름이 다음
// 줄인 선언). 표지 주석(`/*@이름*/`)과 그 뜻은 ../../README.md 「앵커 표」. 이름은 `M416` 으로 시작한다.

/** 표준 데코레이터(TS 5+ — `experimentalDecorators` 없이). Angular 의 `@Component({ … })` 처럼 인자를 받아 여러 줄에 걸친다. */
function m416Component(_options: { selector: string }) {
  return <T>(target: T, _context: ClassDecoratorContext): T => target;
}

@m416Component({
  selector: "m416-panel",
})
export class /*@sym.decorated*/M416Panel {
  size = 1;
}

export class
  /*@sym.split*/M416Split {
  size = 2;
}
