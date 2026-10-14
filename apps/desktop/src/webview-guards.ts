// 웹뷰가 브라우저처럼 구는 기본 동작(새로고침·줌·뒤로가기·기본 컨텍스트 메뉴)을 막는다.
// 웹뷰 설정만으로 끌 수 있는 것(줌·스와이프)은 Rust 쪽(src-tauri/src/windows.rs)이 끄고, 여기서는 설정이 없는 것을
// 막는다. Tauri 는 WebView2 의 브라우저 가속키(F5·Ctrl+R·Alt+←)를 끄는 설정을 노출하지 않고, WKWebView 의
// 기본 컨텍스트 메뉴에는 「Reload」가 있다. macOS 에서 ⌘R·F5·⌘= 는 이 핸들러를 빼도 아무 효과가 없었다
// (debug 빌드에서 osascript 로 키를 보내 vite 의 /index.html 요청 수와 화면 캡처로 확인).
//
// preventDefault 만 하고 stopPropagation 은 하지 않는다. 이 핸들러는 window 의 버블 단계에 붙으므로
// 터미널·에디터처럼 같은 키를 자기 용도로 쓰는 컴포넌트는 먼저 이벤트를 받는다.

export interface KeyLike {
  code: string;
  key: string;
  metaKey: boolean;
  ctrlKey: boolean;
  altKey: boolean;
}

export interface Platform {
  mac: boolean;
}

// 글자 키는 code 로 본다. 한글 입력 상태에서는 ⌘R 의 key 가 "ㄱ" 으로 들어온다.
const RELOAD_CODES = new Set(["KeyR"]);
const ZOOM_CODES = new Set(["Equal", "Minus", "Digit0", "NumpadAdd", "NumpadSubtract", "Numpad0"]);
const HISTORY_CODES = new Set(["BracketLeft", "BracketRight"]);
const HISTORY_KEYS = new Set(["BrowserBack", "BrowserForward"]);

export function shouldBlockKey(event: KeyLike, platform: Platform): boolean {
  if (event.key === "F5" || HISTORY_KEYS.has(event.key)) return true;

  // ⌘ 는 macOS, Ctrl 은 그 밖의 OS 에서만 본다. macOS 의 Ctrl+R 은 웹뷰 동작이 아니라 셸·편집기 키다.
  const primary = platform.mac ? event.metaKey : event.ctrlKey;
  if (primary && !event.altKey) {
    if (RELOAD_CODES.has(event.code) || ZOOM_CODES.has(event.code)) return true;
    if (HISTORY_CODES.has(event.code)) return true;
  }

  // Alt+←/→ 는 Windows·Linux 웹뷰의 뒤로/앞으로다. macOS 에서는 단어 이동이라 건드리지 않는다.
  if (!platform.mac && event.altKey && !event.ctrlKey && !event.metaKey) {
    if (event.key === "ArrowLeft" || event.key === "ArrowRight") return true;
  }
  return false;
}

// 핀치와 Ctrl+휠은 둘 다 ctrlKey 가 켜진 wheel 이벤트로 들어온다.
export function shouldBlockWheel(event: { ctrlKey: boolean }): boolean {
  return event.ctrlKey;
}

// 마우스 4·5번 버튼(MouseEvent.button 3·4)은 뒤로/앞으로다.
export function shouldBlockMouseButton(event: { button: number }): boolean {
  return event.button === 3 || event.button === 4;
}

type Listen = (
  type: string,
  listener: (event: Event) => void,
  options?: AddEventListenerOptions,
) => void;

export function installWebviewGuards(target: { addEventListener: Listen }, platform: Platform) {
  target.addEventListener("keydown", (event) => {
    if (shouldBlockKey(event as KeyboardEvent, platform)) event.preventDefault();
  });
  // passive 가 기본인 wheel 에서는 preventDefault 가 무시되므로 명시적으로 끈다.
  target.addEventListener(
    "wheel",
    (event) => {
      if (shouldBlockWheel(event as WheelEvent)) event.preventDefault();
    },
    { passive: false },
  );
  for (const type of ["mousedown", "mouseup", "auxclick"]) {
    target.addEventListener(type, (event) => {
      if (shouldBlockMouseButton(event as MouseEvent)) event.preventDefault();
    });
  }
  // 기본 메뉴에는 「새로고침」과 「요소 검사」가 들어 있다. 개발자 도구는 Tauri 가 debug 빌드에 넣는
  // 단축키(macOS ⌥⌘I, 그 밖 Ctrl+Shift+I)로 연다 — 그 키는 위 shouldBlockKey 가 건드리지 않는다.
  target.addEventListener("contextmenu", (event) => event.preventDefault());
}

export function detectPlatform(userAgent: string): Platform {
  return { mac: /Mac/.test(userAgent) };
}
