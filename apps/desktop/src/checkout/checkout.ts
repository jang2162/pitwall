// 창의 체크아웃(18-m7-plan M7-11, 08-remote-mobile.md 「창과 활성 체크아웃」): 묶음 창이 보는 체크아웃은 (머신, 워크트리)다. 창 안의
// 탭 · 탭 묶음 · 문서 · 활성 워크트리는 지금까지 워크트리 꼭대기 하나(로컬 경로, 메인 체크아웃은 `undefined`)를 열쇠로 써 왔다.
// 다른 머신의 체크아웃도 같은 자리에 들도록 **다른 머신의 경로에는 머신을 앞에 붙인다**(`pitwall-remote://<머신>/경로`):
//
// - 탭의 `path` · `worktree`, 탭 묶음의 열쇠(`WorktreeKey`), 문서 열쇠가 모두 이 꼴이라 로컬과 다른 머신의 같은 경로가 섞이지 않는다
//   — 두 머신의 `/home/me/app/a.ts` 는 다른 문서다.
// - 이 머신의 경로는 그대로다 — 옛 저장본 · 로컬만인 창은 바뀌지 않는다.
// - 다른 머신의 메인 체크아웃(워크트리 없음)의 열쇠는 경로가 빈 `pitwall-remote://<머신>` 이다.
//
// 열쇠를 Host 에 보내기 전에 [`unqualify`] 로 (머신, 그 머신의 경로)로 푼다 — Host 는 이 꼴을 모른다.

/** 이 머신의 머신 id(`pw_core::registry::bundle::LOCAL_MACHINE`, 라우터의 로컬 Host id). */
export const LOCAL_MACHINE = "local";

const REMOTE = "pitwall-remote://";

/** 체크아웃 식별자(`CheckoutRef` 와 같은 꼴). `worktree` 가 없으면 그 머신 멤버의 메인 체크아웃이다. */
export interface Checkout {
  machine: string;
  worktree?: string;
}

/** 그 머신의 경로 → 창 안의 경로. 이 머신이면 그대로다. */
export function qualify(machine: string, path: string): string {
  return machine === LOCAL_MACHINE ? path : `${REMOTE}${encodeURIComponent(machine)}${path}`;
}

/** 창 안의 경로 → (머신, 그 머신의 경로). 접두가 없으면 이 머신의 경로다. */
export function unqualify(path: string): { machine: string; path: string } {
  if (!path.startsWith(REMOTE)) return { machine: LOCAL_MACHINE, path };
  const rest = path.slice(REMOTE.length);
  const slash = rest.indexOf("/");
  const machine = decodeURIComponent(slash < 0 ? rest : rest.slice(0, slash));
  return { machine, path: slash < 0 ? "" : rest.slice(slash) };
}

/** 그 경로가 이 머신의 것이 아닌가. */
export function isRemotePath(path: string | undefined): boolean {
  return path !== undefined && path.startsWith(REMOTE);
}

/** 체크아웃 → 탭 묶음 · 활성 워크트리의 열쇠. 이 머신의 메인 체크아웃이면 `undefined`(옛 열쇠 그대로). */
export function checkoutKey(checkout: Checkout): string | undefined {
  if (checkout.machine === LOCAL_MACHINE) return checkout.worktree;
  return qualify(checkout.machine, checkout.worktree ?? "");
}

/** [`checkoutKey`] 의 반대. */
export function checkoutOfKey(key: string | undefined): Checkout {
  if (key === undefined) return { machine: LOCAL_MACHINE };
  const { machine, path } = unqualify(key);
  return path === "" ? { machine } : { machine, worktree: path };
}

/** 탭 · 문서의 머신. 경로에 접두가 없으면 워크트리 열쇠를 본다(둘 다 없으면 이 머신). */
export function machineOf(path: string, worktree?: string): string {
  if (isRemotePath(path)) return unqualify(path).machine;
  return checkoutOfKey(worktree).machine;
}
