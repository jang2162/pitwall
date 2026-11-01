// 미사용 import(진단 · 코드 액션 「import 지우기」).

// 저장소의 oxlint 가 이 의도된 미사용을 막지 않게(서버의 진단 TS6133 은 그대로 난다).
// eslint-disable-next-line no-unused-vars
import { /*@unused.rect*/Rect } from "./shapes";

/** 아무것도 쓰지 않는다. */
export function nothing(): void {}
