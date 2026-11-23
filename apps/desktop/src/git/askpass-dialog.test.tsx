// @vitest-environment happy-dom
// 원격 askpass 다이얼로그의 머신 이름(18-m7-plan M7-15 웹뷰 시험): 다른 머신의 git · ssh 가 낸 질문이면 다이얼로그에 그 머신 이름이
// 글로 보이고, 이 머신의 질문이면 없다. 앱(main.tsx)처럼 StrictMode 로 그린다.

import { LocalizationProvider } from "@fluent/react";
import { act, StrictMode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

import { bundledLocalization } from "../i18n/testing";
import { AskpassDialog } from "./git-prompts";

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

function render(machine: string | undefined, locale = "ko") {
  const container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
  act(() =>
    root!.render(
      <StrictMode>
        <LocalizationProvider l10n={bundledLocalization(locale)}>
          <AskpassDialog
            prompt={{
              promptId: 1,
              text: "Password for 'https://me@git.example': ",
              hint: "secret",
            }}
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
