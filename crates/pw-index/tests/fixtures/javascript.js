// 심볼 인덱스 픽스처 — 바꾸면 스냅숏(tests/snapshots/)도 바뀐다.

export class Counter {
  constructor(start) {
    this.value = start;
  }

  increment() {
    this.value += 1;
    return this.value;
  }
}

export function makeCounter(start) {
  return new Counter(start);
}

export const double = (x) => x * 2;

export const handlers = {
  onClick: function () {
    return "click";
  },
  onHover: () => "hover",
};

function* ids() {
  yield 1;
}

export default ids;
