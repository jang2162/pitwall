// 자동완성 · 자동 import(15-m4-plan.md M4-11 live). `obj.` 바로 뒤의 완성 후보에 `reading` · `limit` 이 든다. 자동 import 는 시험이
// 표지 자리에 `scal` 을 편집으로 넣고 그 끝에서 묻는다 — 후보 `scale`(./shapes — 이 파일은 import 하지 않았다)을 resolve 하면 import
// 줄이 온다. 파일 자체에는 오류가 없다(다른 행의 진단 수에 들지 않게).

interface Meter {
  reading: number;
  limit: number;
}

/** 남은 양. */
export function headroom(obj: Meter): number {
  return obj./*@m4-11.obj-members*/limit - obj.reading;
}

/** 자동 import 자리. */
export function auto(): number {
  return /*@m4-11.auto-import*/0;
}
