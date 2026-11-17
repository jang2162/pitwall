// 웹뷰 시험이 그리는 모든 React 루트를 앱(main.tsx)처럼 `<StrictMode>` 로 감싼다(vite.config.ts 의 `test.setupFiles`).
//
// StrictMode(dev)는 마운트 직후 effect 를 한 번 정리했다 다시 건다. `useState` 로 한 번 만든 모델에 정리(`dispose`)만 걸면 다시 걸
// 때 되살릴 길이 없어 dev 앱에서 멈춘다 — 시험이 StrictMode 없이 그리면 이것을 못 잡는다(M5-13b: 호출 로그가 「Loading…」 에,
// Search Everywhere 가 빈 결과에 멈췄다). 시험 파일마다 감싸지 않고 `createRoot` 에서 감싸 기본으로 둔다.

import { createElement, type ReactNode, StrictMode } from "react";
import { vi } from "vitest";

vi.mock("react-dom/client", async (importOriginal) => {
  const real = await importOriginal<typeof import("react-dom/client")>();
  const createRoot: typeof real.createRoot = (container, options) => {
    const root = real.createRoot(container, options);
    const render = root.render.bind(root);
    root.render = (children: ReactNode) => render(createElement(StrictMode, null, children));
    return root;
  };
  return { ...real, default: { ...real, createRoot }, createRoot };
});
