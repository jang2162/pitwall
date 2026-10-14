import { describe, expect, it } from "vitest";

import {
  type KeyLike,
  detectPlatform,
  installWebviewGuards,
  shouldBlockKey,
  shouldBlockMouseButton,
  shouldBlockWheel,
} from "./webview-guards";

const mac = { mac: true };
const other = { mac: false };

function key(code: string, init: Partial<KeyLike> = {}): KeyLike {
  return { code, key: init.key ?? "", metaKey: false, ctrlKey: false, altKey: false, ...init };
}

describe("shouldBlockKey", () => {
  it("blocks reload on every platform", () => {
    expect(shouldBlockKey(key("KeyR", { metaKey: true }), mac)).toBe(true);
    expect(shouldBlockKey(key("KeyR", { ctrlKey: true }), other)).toBe(true);
    expect(shouldBlockKey(key("F5", { key: "F5" }), mac)).toBe(true);
    expect(shouldBlockKey(key("F5", { key: "F5" }), other)).toBe(true);
  });

  it("matches the physical key, so a Korean IME does not slip ⌘R through", () => {
    expect(shouldBlockKey(key("KeyR", { key: "ㄱ", metaKey: true }), mac)).toBe(true);
  });

  it("blocks zoom shortcuts", () => {
    for (const code of ["Equal", "Minus", "Digit0", "NumpadAdd", "NumpadSubtract", "Numpad0"]) {
      expect(shouldBlockKey(key(code, { metaKey: true }), mac)).toBe(true);
      expect(shouldBlockKey(key(code, { ctrlKey: true }), other)).toBe(true);
    }
  });

  it("blocks history navigation keys", () => {
    expect(shouldBlockKey(key("BracketLeft", { metaKey: true }), mac)).toBe(true);
    expect(shouldBlockKey(key("BracketRight", { metaKey: true }), mac)).toBe(true);
    expect(shouldBlockKey(key("ArrowLeft", { key: "ArrowLeft", altKey: true }), other)).toBe(true);
    expect(shouldBlockKey(key("ArrowRight", { key: "ArrowRight", altKey: true }), other)).toBe(
      true,
    );
    expect(shouldBlockKey(key("BrowserBack", { key: "BrowserBack" }), other)).toBe(true);
  });

  it("leaves keys that mean something else on that platform alone", () => {
    // macOS 의 Ctrl+R 은 셸 역검색, ⌥← 는 단어 이동이다.
    expect(shouldBlockKey(key("KeyR", { ctrlKey: true }), mac)).toBe(false);
    expect(shouldBlockKey(key("ArrowLeft", { key: "ArrowLeft", altKey: true }), mac)).toBe(false);
    expect(shouldBlockKey(key("KeyR"), mac)).toBe(false);
    expect(shouldBlockKey(key("KeyC", { metaKey: true }), mac)).toBe(false);
    // 개발자 도구 단축키(Tauri 가 debug 빌드에 넣는다)는 막지 않는다.
    expect(shouldBlockKey(key("KeyI", { metaKey: true, altKey: true }), mac)).toBe(false);
    expect(shouldBlockKey(key("KeyI", { ctrlKey: true }), other)).toBe(false);
  });
});

describe("pointer guards", () => {
  it("blocks ctrl+wheel (pinch and zoom wheel) only", () => {
    expect(shouldBlockWheel({ ctrlKey: true })).toBe(true);
    expect(shouldBlockWheel({ ctrlKey: false })).toBe(false);
  });

  it("blocks the back and forward mouse buttons only", () => {
    expect(shouldBlockMouseButton({ button: 3 })).toBe(true);
    expect(shouldBlockMouseButton({ button: 4 })).toBe(true);
    expect(shouldBlockMouseButton({ button: 0 })).toBe(false);
    expect(shouldBlockMouseButton({ button: 2 })).toBe(false);
  });
});

describe("installWebviewGuards", () => {
  function dispatch(target: EventTarget, type: string, props: object) {
    const event = Object.assign(new Event(type, { cancelable: true }), props);
    target.dispatchEvent(event);
    return event.defaultPrevented;
  }

  it("cancels the default action of blocked events and nothing else", () => {
    const target = new EventTarget();
    installWebviewGuards(target, mac);
    const plain = { metaKey: false, ctrlKey: false, altKey: false };

    expect(dispatch(target, "keydown", { ...plain, code: "KeyR", key: "r", metaKey: true })).toBe(
      true,
    );
    expect(dispatch(target, "keydown", { ...plain, code: "KeyR", key: "r" })).toBe(false);
    expect(dispatch(target, "wheel", { ctrlKey: true })).toBe(true);
    expect(dispatch(target, "wheel", { ctrlKey: false })).toBe(false);
    expect(dispatch(target, "mouseup", { button: 3 })).toBe(true);
    expect(dispatch(target, "auxclick", { button: 4 })).toBe(true);
    expect(dispatch(target, "mousedown", { button: 0 })).toBe(false);
    expect(dispatch(target, "contextmenu", {})).toBe(true);
  });

  it("registers wheel as non-passive so preventDefault takes effect", () => {
    const options: Record<string, AddEventListenerOptions | undefined> = {};
    installWebviewGuards(
      { addEventListener: (type, _listener, opts) => (options[type] = opts) },
      mac,
    );
    expect(options.wheel?.passive).toBe(false);
  });
});

describe("detectPlatform", () => {
  it("recognises the WKWebView user agent as macOS", () => {
    const wkwebview =
      "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko)";
    expect(detectPlatform(wkwebview)).toEqual(mac);
    expect(detectPlatform("Mozilla/5.0 (Windows NT 10.0; Win64; x64) Edg/140.0")).toEqual(other);
  });
});
