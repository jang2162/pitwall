// 파일을 터미널로 드롭 → 경로 붙여넣기(10-ui.md 「5.7 실행 · 터미널」). 웹뷰의 HTML 드롭은 경로를 주지 않으므로 Tauri 의 네이티브
// 드롭 이벤트(경로 + 창 안 위치)를 받아, 위치가 보이는 터미널 위면 경로를 그 셸의 문법으로 감싸 그 터미널에 붙여 넣는다.
//
// 따옴표 규칙은 셸마다 다르다 — POSIX 셸의 작은따옴표는 이스케이프가 없지만 fish 는 그 안에서 `\'` · `\\` 를 읽으므로, POSIX 식으로
// 감싼 이름(`x\';echo pwned;#`)이 fish 에서 따옴표를 빠져나온다. 그래서 셸(`started` 의 argv[0])을 보고 고르고 — argv[0] 는 맨 바깥 셸일 뿐이라 그 안에서 뜬 fish 도 읽도록 POSIX 꼴은 작은따옴표 안에 `\` 를 두지 않는다 — 모르는 셸이나 아직 뜨지
// 않은 셸에는 어느 셸에서도 그대로인 글자의 경로만 붙인다. 제어 문자(개행 · ESC 등)가 든 경로는 어느 셸에도 붙이지 않는다 — 개행은
// bracketed paste 가 꺼진 셸에서 Enter 가 되고, ESC 는 bracketed paste 를 끝낼 수 있다.

/** 붙여 넣을 셸의 따옴표 규칙. */
export type ShellQuoting = "posix" | "fish" | "powershell" | "unknown";

const POSIX_SHELLS = new Set(["sh", "bash", "zsh", "dash", "ksh", "mksh", "ash"]);

/** 셸 실행 파일(argv[0])의 따옴표 규칙. 없거나(셸이 아직 뜨지 않음) 모르는 셸(nu · xonsh · cmd 등)은 `"unknown"`. */
export function shellQuoting(program: string | undefined): ShellQuoting {
  if (!program) return "unknown";
  const name = (program.split(/[\\/]/).pop() ?? "")
    .replace(/^-/, "")
    .toLowerCase()
    .replace(/\.exe$/, "");
  if (POSIX_SHELLS.has(name)) return "posix";
  if (name === "fish") return "fish";
  if (name === "pwsh" || name === "powershell") return "powershell";
  return "unknown";
}

// 따옴표 없이 써도 한 낱말이고 펼쳐지지 않는 글자. POSIX: 맨 앞의 `=` 는 zsh 가 명령 경로로 펼친다(EQUALS). fish: `%` 는 옛 fish 의
// 프로세스 펼침(POSIX 꼴도 안에서 뜬 fish 가 읽으므로 `%` 는 감싼다). 모르는 셸: 어느 셸에서도 글자 그대로인 것만.
const PLAIN: Record<ShellQuoting, RegExp> = {
  posix: /^[A-Za-z0-9_\-./,:@+][A-Za-z0-9_\-./,:@+=]*$/,
  fish: /^[A-Za-z0-9_\-./,:@+=]+$/,
  powershell: /^[A-Za-z0-9_\-.\\/:]+$/,
  unknown: /^[A-Za-z0-9_\-./]+$/,
};

function hasControl(text: string): boolean {
  for (const ch of text) {
    const code = ch.codePointAt(0)!;
    if (code < 0x20 || (code >= 0x7f && code <= 0x9f)) return true;
  }
  return false;
}

/** 셸 한 낱말로 감싼다. 이 셸에 안전하게 붙일 수 없으면 `undefined`. */
export function quoteForShell(path: string, quoting: ShellQuoting): string | undefined {
  if (path === "" || hasControl(path)) return undefined;
  if (PLAIN[quoting].test(path)) return path;
  switch (quoting) {
    case "posix":
      // 작은따옴표 안에 `\` 를 두지 않는다 — 안에서 `\` 를 읽는 fish 가 같은 글자를 받는 경우(bash · zsh 안에서 뜬 fish)에도 같은 뜻이
      // 되도록, `'` 와 `\` 는 닫고 따옴표 밖의 `\'` · `\\` 로 넣은 뒤 다시 연다. 한 번에 바꾼다(두 번 나눠 바꾸면 앞서 넣은 `\` 를 또 바꾼다).
      return `'${path.replace(/['\\]/g, (c) => (c === "'" ? `'\\''` : `'\\\\'`))}'`;
    case "fish":
      // 작은따옴표 안에서 `\` 와 `'` 만 이스케이프를 받는다(fish 문서 「Quotes」).
      return `'${path.replace(/[\\']/g, (c) => `\\${c}`)}'`;
    case "powershell":
      // 작은따옴표 문자열은 펼치지 않는다. 따옴표로 치는 네 글자(' ‘ ’ ‚ ‛ 중 하나)는 같은 글자를 겹쳐 넣는다.
      return `'${path.replace(/['\u2018\u2019\u201A\u201B]/g, (c) => c + c)}'`;
    case "unknown":
      return undefined;
  }
}

/** 붙여 넣을 글자: 경로를 공백으로 잇고 끝에 공백 하나(이어서 칠 수 있게). 하나라도 붙일 수 없으면 `undefined`. */
export function dropText(paths: readonly string[], quoting: ShellQuoting): string | undefined {
  if (paths.length === 0) return "";
  const quoted = paths.map((p) => quoteForShell(p, quoting));
  if (quoted.some((q) => q === undefined)) return undefined;
  return `${quoted.join(" ")} `;
}

/** 드롭 이벤트의 위치(물리 픽셀)가 요소의 사각형(CSS 픽셀) 안인가. */
export function dropHits(
  rect: Pick<DOMRect, "left" | "top" | "right" | "bottom">,
  position: { x: number; y: number },
  scale: number,
): boolean {
  const x = position.x / scale;
  const y = position.y / scale;
  return x >= rect.left && x < rect.right && y >= rect.top && y < rect.bottom;
}
