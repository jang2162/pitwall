// 시간 초과한 시험의 본문은 멈추지 않고 계속 돈다 — 그대로 두면 다음 시험의 act · DOM · inbox 와 겹쳐 같은 파일의 시험이 줄줄이
// 실패한다(부하에서 26 개). 시험마다 토큰을 달고, 끝난 시험이 `act` 를 부르면 던져서 본문을 끊는다. 시험 파일은 react 의 `act` 와
// vitest 의 `it` 대신 여기 것을 쓰고, `afterEach` 맨 앞에서 `endTest()` 를 기다린다.

import { AsyncLocalStorage } from "node:async_hooks";

import { act as reactAct } from "react";
import { it as vitestIt } from "vitest";

const testToken = new AsyncLocalStorage<{ ended: boolean }>();
let liveToken: { ended: boolean } | undefined;
const inFlight = new Set<PromiseLike<unknown>>();

/** react 의 `act`. 끝난 시험의 본문이 부르면 던진다. */
export const act = ((callback: Parameters<typeof reactAct>[0]) => {
  if (testToken.getStore()?.ended) throw new Error("이 시험은 이미 끝났다");
  const result = reactAct(callback as never) as PromiseLike<unknown> | undefined;
  if (typeof result?.then === "function") {
    inFlight.add(result);
    const done = () => void inFlight.delete(result);
    void result.then(done, done);
  }
  return result;
}) as typeof reactAct;

/** vitest 의 `it`(본문만 받는다). 본문이 끝났는지를 `act` 가 안다. */
export const it = (name: string, body: () => Promise<void>) =>
  vitestIt(name, () => {
    const token = { ended: false };
    liveToken = token;
    return testToken.run(token, body);
  });

/** `it.each` 꼴. */
it.each =
  <T>(cases: readonly T[]) =>
  (name: string, body: (item: T) => Promise<void>) =>
    vitestIt.each(cases)(name, (item) => {
      const token = { ended: false };
      liveToken = token;
      return testToken.run(token, () => body(item));
    });

/** 시험이 끝났음을 알리고, 진행 중인 `act` 가 마치기를 기다린다(영영 안 끝나도 1 초만). `afterEach` 에서 부른다. */
export async function endTest() {
  if (liveToken) liveToken.ended = true;
  await Promise.race([Promise.allSettled(inFlight), new Promise((r) => setTimeout(r, 1000))]);
  inFlight.clear();
}
