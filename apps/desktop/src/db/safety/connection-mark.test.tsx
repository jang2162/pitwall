// @vitest-environment happy-dom
// 연결 표지(M3-19 ① ④): 프로덕션은 `color` 와 상관없이 빨강(`db.production`)이고 색과 함께 글자 라벨이 있다(접근성), 설정의 색은
// `db.tag.*` 이름 · `#rrggbb` 만 받는다, TLS 표지는 검증하지 않은 TLS · 평문에서만 보인다.

import { LocalizationProvider } from "@fluent/react";
import type { TlsStatus } from "@pitwall/rpc-client";
import type { ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect } from "vitest";

import { act, endTest, it } from "../../test-support/ended-test";
import { bundledLocalization, bundledTranslator } from "../../i18n/testing";
import { DB_TAG_TOKENS } from "../../theme/tokens";
import {
  ConnectionMark,
  connectionColor,
  connectionFrameStyle,
  connectionLabels,
  TlsMark,
} from "./connection-mark";
import { dbSafetyStrings } from "./strings";

declare global {
  var IS_REACT_ACT_ENVIRONMENT: boolean;
}

const T = dbSafetyStrings(bundledTranslator("ko"));
let root: Root | undefined;

beforeEach(() => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
});

afterEach(async () => {
  await endTest();
  act(() => root?.unmount());
  root = undefined;
  document.body.innerHTML = "";
});

async function render(node: ReactNode) {
  const element = document.createElement("div");
  document.body.append(element);
  root = createRoot(element);
  await act(async () =>
    root!.render(
      <LocalizationProvider l10n={bundledLocalization("ko")}>{node}</LocalizationProvider>,
    ),
  );
  return element;
}

describe("connectionColor", () => {
  it("paints a production connection red whatever its color says", async () => {
    for (const color of [undefined, null, "db.tag.green", "#00ff00"])
      expect(connectionColor({ production: true, color })).toBe("var(--pw-db-production)");
  });

  it("takes a db.tag token or #rrggbb and drops anything else", async () => {
    for (const tag of DB_TAG_TOKENS)
      expect(connectionColor({ color: tag })).toBe(`var(--pw-${tag.replaceAll(".", "-")})`);
    expect(connectionColor({ color: "#1a2B3c" })).toBe("#1a2B3c");
    expect(connectionColor({ color: " #1a2b3c " })).toBe("#1a2b3c");
    for (const bad of [
      "",
      "red",
      "#fff",
      "#1a2b3c80",
      "ui.background",
      "db.production",
      "db.tag.pink",
      "#000000;background:url(https://x)",
      "var(--pw-db-production)",
    ])
      expect(connectionColor({ color: bad }), bad).toBeUndefined();
    expect(connectionColor({})).toBeUndefined();
  });

  it("frames tabs and consoles in the connection color, thicker for production", async () => {
    expect(connectionFrameStyle({ color: "db.tag.blue" })).toEqual({
      borderColor: "var(--pw-db-tag-blue)",
      borderWidth: 1,
      borderStyle: "solid",
    });
    expect(connectionFrameStyle({ production: true })).toMatchObject({
      borderColor: "var(--pw-db-production)",
      borderWidth: 2,
    });
    expect(connectionFrameStyle({ readOnly: true })).toBeUndefined();
  });
});

describe("ConnectionMark", () => {
  it("labels production and read-only in words next to the color", async () => {
    expect(connectionLabels({ production: true, readOnly: true }, T.label)).toEqual([
      { kind: "production", text: "프로덕션" },
      { kind: "readOnly", text: "읽기 전용" },
    ]);
    const element = await render(
      <ConnectionMark name="prod-billing" production readOnly color="db.tag.green" />,
    );
    const mark = element.querySelector<HTMLElement>("[data-testid=db-connection-mark]")!;
    expect(mark.dataset.production).toBe("true");
    expect(mark.textContent).toBe("prod-billing프로덕션읽기 전용");
    expect(
      element.querySelector<HTMLElement>("[data-testid=db-connection-color]")!.style
        .backgroundColor,
    ).toBe("var(--pw-db-production)");
  });

  it("has no label and no dot for a plain connection without a color", async () => {
    const element = await render(<ConnectionMark name="local" />);
    expect(element.querySelector("[data-label]")).toBeNull();
    expect(element.querySelector("[data-testid=db-connection-color]")).toBeNull();
    expect(element.textContent).toBe("local");
  });
});

describe("TlsMark", () => {
  it.each([
    { tls: "unverified", text: "서버 인증서를 검증하지 않음" },
    { tls: "plaintext", text: "평문" },
  ] as const)("marks $tls connections", async ({ tls, text }) => {
    const element = await render(<TlsMark tls={tls} />);
    const mark = element.querySelector<HTMLElement>("[data-testid=db-tls-mark]")!;
    expect(mark.dataset.tls).toBe(tls);
    expect(mark.textContent).toBe(text);
    expect(mark.title).not.toBe("");
  });

  it.each(["verified", "notApplicable"] satisfies TlsStatus[])(
    "draws nothing for %s",
    async (tls) => {
      const element = await render(<TlsMark tls={tls} />);
      expect(element.innerHTML).toBe("");
    },
  );
});

// 18-m7-plan M7-12 ③ · M7-11b: 활성 체크아웃이 다른 머신이면 그 자리의 연결 표지(탐색기 · 상태바)에 머신 이름이 글로 붙는다 — 두 머신에
// 같은 이름의 연결이 있다. 이 머신이면 붙지 않고, 켜지 않은 자리(다른 문맥이 머신을 정하는 다이얼로그)는 붙이지 않는다.
describe("ConnectionMark on another machine", () => {
  const mount = async (machine: string, onMachine: boolean) => {
    const { QueryClient, QueryClientProvider } = await import("@tanstack/react-query");
    const { TargetMachineProvider } = await import("../../checkout/machine-mark");
    return render(
      <QueryClientProvider client={new QueryClient()}>
        <TargetMachineProvider machine={machine}>
          <ConnectionMark name="prod-billing" production onMachine={onMachine} />
        </TargetMachineProvider>
      </QueryClientProvider>,
    );
  };

  it("names the machine only when the checkout is on another machine and the place asks for it", async () => {
    const remote = await mount("SHA256:mini", true);
    expect(remote.querySelector("[data-machine]")?.textContent).toBe("SHA256:mini");
    expect(remote.querySelector("[data-machine]")?.getAttribute("aria-label")).toBe(
      "머신: SHA256:mini",
    );
    act(() => root?.unmount());
    document.body.innerHTML = "";
    expect((await mount("local", true)).querySelector("[data-machine]")).toBeNull();
    act(() => root?.unmount());
    document.body.innerHTML = "";
    expect((await mount("SHA256:mini", false)).querySelector("[data-machine]")).toBeNull();
  });
});
