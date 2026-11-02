// LSP 기능 `installBanner`(15-m4-plan M4-17 — `LSP_FEATURES` 의 한 줄): 열린 문서의 서버가 띄워지지 못한 까닭을 탭 위 배너(에디터 위쪽
// 패널 — 10-ui 「빈 상태 · 오류 상태」 의 배너 꼴, 디스크 배너와 같은 모양)로 보인다.
//
// - `notInstalled`(설치 패키지가 있고 `[lsp.<id>] install_prompt` 가 꺼지지 않음): 「<언어> 언어 서버 <이름>(<라이선스>)가 없다 —
//   [설치] [설정] [이 프로젝트에서 묻지 않기]」. [설치] 는 확인 흐름(라이선스 · 출처 다이얼로그 — 처음 한 번)을 지나 설치하고 그 서버를
//   다시 띄운다(허가를 다시 받는다). 「묻지 않기」 는 개인 파일(`project.local.toml`)에만 쓴다.
// - `runtimeMissing`: 「서버용 <도구> <판> 이상이 필요하다 — `mise install <도구>@<판>`(프로젝트 고정은 바꾸지 않는다)」. 설치
//   단추가 없다 — Pitwall 은 런타임을 설치하지 않는다(D26 · 결정 M4-D6).
// - `untrusted`: 「프로젝트를 신뢰해야 언어 서버를 켠다 [신뢰…]」.
//
// 상태는 창의 상태 표(status-window.ts)를 `doc.services` 로 찾아 본다 — 창이 붙지 않았으면(시험 · 파일 창) 배너가 없다. 라이선스 ·
// 서버 이름 같은 registry · 설정의 글은 `textContent` 로만 넣는다(HTML 로 해석하지 않는다).

import { type Panel, showPanel } from "@codemirror/view";

import type { LspEditorFeature } from "./features";
import type { LspDocument } from "./lsp-sessions";
import { type LspStatusWindow, docKey, statusWindowOf } from "./status-window";
import { runtimeHint } from "../tools/install";

const BUTTON =
  "flex-none rounded-sm border border-ui-border px-2 py-px text-ui-foreground hover:bg-ui-hover disabled:opacity-50";

export interface BannerView {
  kind: "notInstalled" | "runtimeMissing" | "untrusted";
  server: string;
  text: string;
  /** 단추(라벨, 동작). */
  buttons: { id: string; label: string; disabled?: boolean; run(): void }[];
}

/** 이 문서의 배너. 보일 것이 없으면 `undefined`. */
export function bannerFor(win: LspStatusWindow, doc: LspDocument): BannerView | undefined {
  const state = win.store.getState();
  const tracked = state.docs[docKey(doc)];
  const T = win.strings();
  const instances = tracked?.status?.instances ?? [];
  const untrusted = instances.find((i) => i.state.kind === "untrusted");
  if (untrusted)
    return {
      kind: "untrusted",
      server: untrusted.server,
      text: T.untrusted,
      buttons: [{ id: "trust", label: T.trust, run: () => win.trust() }],
    };
  for (const instance of instances) {
    const s = instance.state;
    if (s.kind === "runtimeMissing") {
      const hint = runtimeHint(s.tool, s.needed);
      return {
        kind: "runtimeMissing",
        server: instance.server,
        text: T.runtimeMissing(hint.name, s.needed, hint.install),
        buttons: [],
      };
    }
    if (s.kind === "notInstalled" && s.mason) {
      const info = win.serverInfo(instance.server, doc.worktree);
      // 설정을 아직 읽지 못했으면 기다린다(묻지 않기로 한 서버를 잠깐이라도 보이지 않게).
      if (!info || !info.installPrompt) continue;
      const licenses = state.licenses[s.mason];
      const installing = state.installing[instance.server] === true;
      const server = instance.server;
      return {
        kind: "notInstalled",
        server,
        text: installing
          ? T.banner.installing(server)
          : T.banner.notInstalled(
              doc.language,
              server,
              licenses && licenses.length > 0 ? licenses.join(" · ") : undefined,
            ),
        buttons: [
          {
            id: "install",
            label: T.banner.install,
            disabled: installing,
            run: () => void win.install(server, doc.worktree),
          },
          { id: "settings", label: T.banner.settings, run: () => win.openSettings() },
          {
            id: "dontAsk",
            label: T.banner.dontAsk,
            disabled: installing,
            run: () => void win.dontAsk(server, doc.worktree),
          },
        ],
      };
    }
  }
  return undefined;
}

function bannerPanel(doc: LspDocument): Panel {
  const dom = document.createElement("div");
  dom.setAttribute("role", "status");
  dom.dataset.testid = "lsp-banner";
  dom.className =
    "pw-lsp-banner flex flex-none flex-wrap items-center gap-2 border-b border-ui-accent bg-ui-panel px-2 py-1 text-[12px]";
  dom.hidden = true;
  const win = statusWindowOf(doc.services);
  let release: (() => void) | undefined;
  let unsubscribe: (() => void) | undefined;
  let shown = "";
  const render = () => {
    if (!win) return;
    const view = bannerFor(win, doc);
    // 같은 내용이면 다시 그리지 않는다(포커스를 잃지 않게).
    const signature = view
      ? JSON.stringify([view.kind, view.text, view.buttons.map((b) => [b.id, b.label, b.disabled])])
      : "";
    if (signature === shown) return;
    shown = signature;
    dom.replaceChildren();
    dom.hidden = !view;
    if (!view) return;
    dom.dataset.kind = view.kind;
    dom.dataset.server = view.server;
    const text = document.createElement("span");
    text.className = "min-w-0 flex-1";
    text.textContent = view.text;
    dom.append(text);
    for (const b of view.buttons) {
      const button = document.createElement("button");
      button.type = "button";
      button.className = BUTTON;
      button.dataset.action = b.id;
      button.textContent = b.label;
      button.disabled = b.disabled === true;
      button.addEventListener("click", () => b.run());
      dom.append(button);
    }
  };
  return {
    dom,
    top: true,
    mount() {
      if (!win) return;
      release = win.track({ path: doc.path, language: doc.language, worktree: doc.worktree });
      unsubscribe = win.store.subscribe(render);
      render();
    },
    destroy() {
      unsubscribe?.();
      release?.();
    },
  };
}

export const installBanner: LspEditorFeature = {
  id: "installBanner",
  extension: (_context, doc) => showPanel.of(() => bannerPanel(doc)),
};
