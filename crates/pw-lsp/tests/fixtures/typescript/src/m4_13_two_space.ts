// 2칸 들여쓰기에 여러 줄 JSDoc 이 달린 파일(M4-13). 웹뷰의 들여쓰기 판정이 2 를 내고(format.test.ts), TS 서버가 2칸으로 포맷하면
// 바뀌지 않는다(live 시험). JSDoc 의 ` * ` 줄이 판정을 1 로 끌고 가 TS 서버가 파일 전체를 1칸으로 다시 들여쓴 일이 있다.

/** Options for {@link describeOptions}. */
export interface /*@format.two-space*/TwoSpaceOptions {
  /**
   * The name.
   */
  name: string;
  /**
   * How many times.
   */
  count: number;
  /**
   * The separator.
   */
  separator: string;
}

/**
 * Describes the options.
 */
export function describeOptions(options: TwoSpaceOptions): string {
  const parts = [options.name];
  for (let i = 0; i < options.count; i++) {
    parts.push(`#${i}`);
  }
  return parts.join(options.separator);
}
