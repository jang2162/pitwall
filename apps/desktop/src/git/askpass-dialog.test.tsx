// @vitest-environment happy-dom
// 원격 askpass 다이얼로그의 머신 이름(18-m7-plan M7-15 웹뷰 시험): 다른 머신의 git · ssh 가 낸 질문이면 다이얼로그에 그 머신 이름이
// 글로 보이고, 이 머신의 질문이면 없다. 앱(main.tsx)처럼 StrictMode 로 그린다.

import { LocalizationProvider } from "@fluent/react";
import { act, StrictMode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

import { bundledLocalization } from "../i18n/testing";
import { AskpassDialog, askpassText, MAX_PROMPT_CHARS } from "./git-prompts";

// 키맵의 팝업 표시는 이 시험이 보는 것이 아니다(키맵 제공자는 Host 연결을 요구한다).
vi.mock("../keymap/keymap-provider", () => ({ useKeymapPopup: () => {} }));

declare global {
  var IS_REACT_ACT_ENVIRONMENT: boolean;
}

let root: Root | undefined;

beforeEach(() => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
});

afterEach(() => {
  act(() => root?.unmount());
  root = undefined;
  document.body.innerHTML = "";
});

function render(
  machine: string | undefined,
  locale = "ko",
  text = "Password for 'https://me@git.example': ",
) {
  const container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
  act(() =>
    root!.render(
      <StrictMode>
        <LocalizationProvider l10n={bundledLocalization(locale)}>
          <AskpassDialog
            prompt={{ promptId: 1, text, hint: "secret" }}
            machine={machine}
            onAnswer={() => {}}
          />
        </LocalizationProvider>
      </StrictMode>,
    ),
  );
}

const machineLine = () =>
  document.querySelector("[data-testid=git-askpass-dialog] [data-testid=git-askpass-machine]");

it("names the remote machine in a remote askpass dialog", () => {
  render("mini");
  expect(machineLine()?.textContent).toBe("mini 의 git · ssh 가 묻는다");
  act(() => root?.unmount());
  root = undefined;
  document.body.innerHTML = "";
  render("mini", "en");
  expect(machineLine()?.textContent).toBe("Asked by git or ssh on mini");
});

it("shows no machine line for a question of this machine", () => {
  render(undefined);
  expect(document.querySelector("[data-testid=git-askpass-dialog]")).not.toBeNull();
  expect(machineLine()).toBeNull();
});

/** 원격(Relay)이 낸 글은 Desktop 도 걷는다(M7-P19 · 리뷰 F5): 제어 · 양방향 문자를 빼고 512 자에서 자른다. 줄바꿈은 남는다. */
it("cleans a remote question on the desktop as well", () => {
  expect(askpassText("\u001b[2JPass\u202Eword\u0085:\tx\nnext")).toBe("[2JPassword: x\nnext");
  const long = askpassText("a".repeat(MAX_PROMPT_CHARS + 10));
  expect(long).toBe(`${"a".repeat(MAX_PROMPT_CHARS)}…`);
  expect(askpassText("ok")).toBe("ok");

  render("mini", "ko", "Pass\u202Eword\u0007 for x: " + "b".repeat(600));
  const description = document.querySelector("[data-testid=git-askpass-dialog]")?.textContent ?? "";
  expect(description).not.toContain("\u202E");
  expect(description).not.toContain("\u0007");
  expect(description).toContain("Password for x: ");
  expect(description).toContain("…");
  expect(description).not.toContain("b".repeat(MAX_PROMPT_CHARS));
});
