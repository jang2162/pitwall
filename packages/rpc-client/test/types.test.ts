// 타입 안전 확인. 아래 `@ts-expect-error` 줄은 `tsc --noEmit`(mise run lint 의 typecheck)에서 에러여야 한다 —
// 에러가 안 나면 「쓰이지 않은 @ts-expect-error」로 tsc 가 실패한다. 호출은 실행하지 않는다.
import { describe, expect, it } from "vitest";

import { createMemoryTransportPair, RpcClient } from "../src";
import { METHODS, type EchoResult, type StreamMethods, type UnaryMethods } from "./fixtures/rpc";

async function typeChecks(client: RpcClient<UnaryMethods, StreamMethods>): Promise<void> {
  const ok: EchoResult = await client.call("fixture.echo", {
    text: "hi",
    repeatCount: 1,
    mode: "plain",
    note: null,
  });
  void ok;
  await client.call("fixture.fail");
  for await (const item of client.stream("fixture.count", { n: 1 })) {
    const n: number = item;
    void n;
  }

  // @ts-expect-error 선언되지 않은 메서드
  await client.call("fixture.nope", null);
  // @ts-expect-error params 필드 타입이 틀림(text 는 string)
  await client.call("fixture.echo", { text: 1, repeatCount: 1, mode: "plain", note: null });
  // @ts-expect-error enum 값이 아님
  await client.call("fixture.echo", { text: "hi", repeatCount: 1, mode: "lower", note: null });
  // @ts-expect-error 필수 params 누락
  await client.call("fixture.echo");
  // @ts-expect-error Rust 의 snake_case 이름은 와이어 이름이 아니다(camelCase)
  await client.call("fixture.echo", { text: "hi", repeat_count: 1, mode: "plain", note: null });
  // @ts-expect-error stream 메서드를 unary 로 부름
  await client.call("fixture.count", { n: 1 });
  // @ts-expect-error unary 메서드를 stream 으로 부름
  client.stream("fixture.echo", { text: "hi", repeatCount: 1, mode: "plain", note: null });
  // @ts-expect-error 결과 타입이 다름
  const wrong: number = await client.call("fixture.echo", {
    text: "hi",
    repeatCount: 1,
    mode: "plain",
    note: null,
  });
  void wrong;
}

describe("generated method types", () => {
  it("expose the declared metadata at runtime", () => {
    void typeChecks;
    expect(Object.keys(METHODS).sort()).toEqual([
      "fixture.count",
      "fixture.echo",
      "fixture.fail",
      "fixture.wait",
    ]);
    expect(METHODS["fixture.wait"]).toMatchObject({
      kind: "unary",
      tier: "exec",
      roles: ["owner", "mobile"],
    });
    expect(METHODS["fixture.count"].kind).toBe("stream");
    const [transport] = createMemoryTransportPair();
    expect(new RpcClient<UnaryMethods, StreamMethods>(transport)).toBeInstanceOf(RpcClient);
  });
});
