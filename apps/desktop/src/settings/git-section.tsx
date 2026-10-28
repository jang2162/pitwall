// 설정 창 「Git」(10-ui.md S7, 13-m2-plan.md M2-20): git 실행 파일의 경로 · 찾은 버전 · 쓰기를 끈 이유(`git.executable`,
// crates/pw-git/src/executable.rs). 설정 창은 Host 연결이고 읽기만 한다 — 경로를 바꾸는 칸은 없다(`[git] path` 는 settings.toml).
// 버전은 쓰기가 쓰는 판정 그대로다(같은 캐시). 찾는 PATH 는 로그인 셸의 것이라 그렇게 적는다.

import type { GitExecutable } from "@pitwall/rpc-client";

import { type GitDialogStrings, useGitDialogStrings } from "../git/dialogs/strings";
import { useRpcQuery } from "../rpc/host";

const BUTTON =
  "rounded-sm border border-ui-border bg-ui-panel px-2 py-0.5 hover:bg-ui-hover disabled:opacity-50";
const NOTE = "text-[12px] text-ui-muted-foreground";

function errorText(e: unknown): string {
  if (typeof e === "object" && e !== null && "message" in e) return String(e.message);
  return String(e);
}

/** 쓰기 줄의 글. */
export function writesText(info: GitExecutable, T: GitDialogStrings["settings"]): string {
  const off = info.writesOff;
  if (!off) return T.writesOn;
  switch (off.kind) {
    case "notFound":
      return T.notFound(off.program);
    case "tooOld":
      return T.tooOld(off.version, info.minimum);
    case "unreadable":
      return T.unreadable(off.reason);
  }
}

export function GitSection() {
  const T = useGitDialogStrings().settings;
  const query = useRpcQuery("git.executable", null, { retry: false });
  const info = query.data;
  return (
    <section aria-labelledby="settings-git" className="flex flex-col gap-2">
      <h3 id="settings-git" className="font-semibold">
        {T.title}
      </h3>
      <div role="status" aria-live="polite" className="flex flex-col gap-1">
        {query.isPending && <span className={NOTE}>{T.checking}</span>}
        {query.isError && (
          <span role="alert" className="text-[12px] text-ui-accent">
            {T.failed(errorText(query.error))}
          </span>
        )}
        {info && (
          <dl className="grid grid-cols-[max-content_1fr] gap-x-3 gap-y-1">
            <dt className="text-ui-muted-foreground">{T.path}</dt>
            <dd>
              <code data-testid="settings-git-program" className="select-all">
                {info.program ?? "—"}
              </code>
              <div className={NOTE}>
                {info.configured !== null ? T.configured(info.configured) : T.notConfigured}
              </div>
            </dd>
            <dt className="text-ui-muted-foreground">{T.version}</dt>
            <dd>
              <span data-testid="settings-git-version">{info.version ?? "—"}</span>
              <div className={NOTE}>{T.minimum(info.minimum)}</div>
            </dd>
            <dt className="text-ui-muted-foreground">{T.writes}</dt>
            <dd
              data-testid="settings-git-writes"
              className={info.writesOff ? "text-ui-accent" : undefined}
            >
              {writesText(info, T)}
            </dd>
          </dl>
        )}
        {info && (
          <span className={NOTE}>
            {info.shellFailure !== null ? T.processPath(info.shellFailure) : T.shellPath}
          </span>
        )}
      </div>
      <div>
        <button
          type="button"
          className={BUTTON}
          disabled={query.isFetching}
          onClick={() => void query.refetch()}
        >
          {T.recheck}
        </button>
      </div>
    </section>
  );
}
