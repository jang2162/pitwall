// 설정 창 「MCP」(10-ui.md S7 · 05-mcp.md 「툴 노출 설정」 의 UI 행 · F10-6, 16-m5-plan M5-15): 프리셋, 등급별 자동 승인, 감사 보관 일수,
// 툴 그룹 토글, 툴 목록(이름 · 설명 · 등급 · 그룹 · 최근 호출 수 · 켜진 까닭)과 검색 · 툴 하나 토글 · 입력 스키마 미리보기, 에이전트
// 등록 문구.
//
// - **편집은 전역 `[mcp]` 만**(결정 M5-D36): 쓰기는 모두 `mcp.settings.set` 이고 Host 가 값을 판정한다 — 화면은 막지 않고 거절의
//   까닭(`McpSettingRefused`)을 번역해 보인다. 프로젝트 설정은 여기서 쓰지 않는다.
// - **판정을 화면에서 다시 하지 않는다**(메인 판정 2026-11-17): 툴의 켜짐과 까닭은 `mcp.tools.list` 의 `enabled` · `rule` · `key`
//   그대로, 프리셋 · 자동 승인 · 그룹 · 보관 일수는 `mcp.settings.get` 이 Host 가 쓰는 값으로 준다.
// - **프로젝트 덮어쓰기 보기**(읽기만): 고른 프로젝트에서의 실제 판정(`mcp.tools.list { projectId }` 의 `enabled` · `rule` · `key`)을
//   툴 줄마다 「이 프로젝트에서: 켜짐/꺼짐 — 까닭」 으로 전역 판정 옆에 나란히 보이고, 두 판정이 다르면 「전역과 다름」 표지를 단다.
//   **전역 토글은 잠그지 않는다**(메인 판정 2026-11-17 — M5-15 리뷰): 전역의 정확한 툴 키는 프로젝트가 그룹 · 프리셋 · glob 으로
//   바꾼 판정에도 닿고(정확한 키가 앞선다), 프로젝트 파일의 정확한 키 · 공유 설정의 좁히기에는 닿지 않는다 — 어느 쪽인지는
//   화면이 추론하지 않고, 고친 뒤 Host 가 다시 낸 「이 프로젝트에서」 줄이 보인다. 그 프로젝트 파일이 `[mcp]` 아래에 적은 값과
//   파일은 `mcp.settings.get { projectId }` 가 준다. 「파일로 열기」 는 Host 가 준 경로만 연다(화면이 조립하지 않는다).
// - 툴 설명은 영어 그대로다(에이전트가 읽는 글 — 결정 M5-D39).
// - **Streamable HTTP**(18-m7-plan M7-25 — 계획 2 의 12번 시나리오): 켜기 · 포트는 `mcp.settings.set`(`httpEnabled` · `httpPort`)이고
//   Host 가 곧바로 맞춰 듣는지 · 포트가 쓰이는 중인지를 돌려준다. 주소는 `http://127.0.0.1:<port>/mcp`(Host 가 준 글). 토큰은 가려
//   두고 [보기] · [복사] 를 누를 때만 `mcp.http.token` 으로 읽는다(화면 상태에 오래 두지 않는다 — 가리면 버린다). 「토큰 새로
//   만들기」 는 한 번 더 묻고 `mcp.http.regenerateToken`. 클라이언트 설정 예는 토큰 자리를 `<token>` 으로 둔다.
// - 에이전트 등록 문구는 복사만 한다 — 자동 등록(F10-5)은 하지 않는다(결정 M5-D33). 문구의 출처는 아래 `REGISTRATIONS`.

import {
  ERROR_CODE,
  type McpHttpStatus,
  type McpSettingKey,
  type McpSettingRefused,
  type McpSettingsView,
  type McpToolInfo,
  type Project,
  RpcCallError,
  type Tier,
} from "@pitwall/rpc-client";
import { useQueryClient } from "@tanstack/react-query";
import { useMemo, useState } from "react";

import { type McpStrings, useMcpStrings } from "../mcp/strings";
import { useHost, useRpcQuery } from "../rpc/host";
import { rpcQueryKey } from "../rpc/queries";
import type { SettingsNative } from "./native";

const BUTTON =
  "rounded-sm border border-ui-border bg-ui-panel px-2 py-0.5 hover:bg-ui-hover disabled:opacity-50";
const NOTE = "text-[12px] text-ui-muted-foreground";
const ALERT = "text-[12px] text-ui-accent";
const SELECT = "rounded-sm border border-ui-border bg-ui-background px-1.5 py-0.5";

type T = McpStrings["settings"];

const PRESETS = ["read-only", "standard", "full"] as const;
/** 자동 승인 칸. `dangerous` 는 없다 — 늘 묻는다(05 「권한 모델」). */
const AUTO_TIERS: readonly Tier[] = ["read", "write", "exec"];

/**
 * 에이전트 클라이언트별 등록 문구(결정 M5-D33 — 복사만). 각 클라이언트의 공식 문서에서 확인한 꼴이다(2026-11-17):
 * - Claude Code: https://code.claude.com/docs/en/mcp — `claude mcp add [options] <name> -- <command> [args...]`, 그 페이지의 stdio
 *   예는 `--transport stdio` 를 적는다(https://code.claude.com/docs/en/mcp-quickstart 는 생략해도 stdio 가 기본이라 적는다 —
 *   명시해 둔다). `--scope user` 는 모든 프로젝트.
 * - Codex: https://developers.openai.com/codex/mcp — `codex mcp add <name> -- <command>`(설정은 `~/.codex/config.toml`).
 * - Cursor: https://cursor.com/docs/mcp — 명령이 없고 `~/.cursor/mcp.json`(전역) · `.cursor/mcp.json`(프로젝트)의 `mcpServers`.
 */
export const REGISTRATIONS = [
  {
    id: "claudeCode",
    text: "claude mcp add --transport stdio --scope user pitwall -- pitwall mcp",
  },
  { id: "codex", text: "codex mcp add pitwall -- pitwall mcp" },
  {
    id: "cursor",
    text: JSON.stringify(
      { mcpServers: { pitwall: { type: "stdio", command: "pitwall", args: ["mcp"] } } },
      null,
      2,
    ),
  },
] as const;

/** 툴을 켜거나 끈 규칙의 글 — `mcp.tools.list` 의 판정 그대로(`rule` · `key`). */
export function ruleText(tool: Pick<McpToolInfo, "rule" | "key">, S: T): string {
  const key = tool.key ?? "";
  switch (tool.rule) {
    case "preset":
      return S.rule.preset(key);
    case "tool":
      return S.rule.tool;
    case "glob":
      return S.rule.glob(key);
    case "group":
      return S.rule.group(key);
    case "shared":
      return S.rule.shared;
  }
}

/** 프로젝트의 판정이 전역 판정과 다른가 — 켜짐 · 규칙 · 키 가운데 하나라도. 표지만 단다(토글은 잠그지 않는다). */
export function differs(global: McpToolInfo, project: McpToolInfo | undefined): boolean {
  return (
    project !== undefined &&
    (project.enabled !== global.enabled ||
      project.rule !== global.rule ||
      project.key !== global.key)
  );
}

/** 검색: 이름 · 그룹 · 설명에 든 글(대소문자 무시). */
export function matches(tool: McpToolInfo, query: string): boolean {
  const q = query.trim().toLowerCase();
  if (q === "") return true;
  return [tool.name, tool.group, tool.description].some((s) => s.toLowerCase().includes(q));
}

/** `mcp.settings.set` 의 오류 글: 거절의 까닭이면 번역하고, 아니면 Host 의 글. */
export function setErrorText(error: unknown, S: T): string {
  if (error instanceof RpcCallError && error.code === ERROR_CODE.INVALID_PARAMS) {
    const reason = (error.data as Partial<McpSettingRefused> | null | undefined)?.reason;
    if (reason && reason in S.refused) return S.refused[reason];
  }
  return S.failed(error instanceof Error ? error.message : String(error));
}

/** 쓰기 하나: 부르고, 바뀐 것을 다시 읽게 하고, 실패면 글. */
function useSetting() {
  const { client, key } = useHost();
  const queryClient = useQueryClient();
  const S = useMcpStrings().settings;
  const [error, setError] = useState<string>();
  const [busy, setBusy] = useState(false);
  const save = async (setting: McpSettingKey, name: string | null, value: unknown) => {
    if (!client) return;
    setBusy(true);
    setError(undefined);
    try {
      await client.call("mcp.settings.set", {
        key: setting,
        name,
        value: value as never,
      });
      // `config.changed` 도 다시 읽게 하지만(rpc/host.tsx) 그 이벤트를 기다리지 않는다.
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: rpcQueryKey(key, "mcp.settings.get") }),
        queryClient.invalidateQueries({ queryKey: rpcQueryKey(key, "mcp.tools.list") }),
      ]);
    } catch (e) {
      setError(setErrorText(e, S));
    } finally {
      setBusy(false);
    }
  };
  return { save, error, busy };
}

function ErrorLine({ error }: { error: string | undefined }) {
  return error ? (
    <span role="alert" className={ALERT}>
      {error}
    </span>
  ) : null;
}

function OpenFile({ path, native }: { path: string; native: SettingsNative }) {
  const S = useMcpStrings().settings;
  const [failed, setFailed] = useState(false);
  if (!native.openFile) return null;
  const open = native.openFile;
  return (
    <>
      <button
        type="button"
        className={BUTTON}
        title={path}
        onClick={() => {
          setFailed(false);
          open(path).catch(() => setFailed(true));
        }}
      >
        {S.openFile}
      </button>
      {failed && (
        <span role="alert" className={ALERT}>
          {S.openFailed(path)}
        </span>
      )}
    </>
  );
}

export function McpSection({ native }: { native: SettingsNative }) {
  const S = useMcpStrings().settings;
  const [projectId, setProjectId] = useState("");
  // 전역 값과 고른 프로젝트의 값을 따로 읽는다 — 프로젝트를 바꾸는 사이에도 전역 칸은 그대로 남는다(다시 그려지지 않는다).
  // 프로젝트가 없으면 두 쿼리는 같은 키라 한 번만 부른다.
  const settings = useRpcQuery("mcp.settings.get", { projectId: null }, { retry: false });
  const ofProject = useRpcQuery(
    "mcp.settings.get",
    { projectId: projectId || null },
    { retry: false },
  );
  const tools = useRpcQuery("mcp.tools.list", {}, { retry: false });
  const view = settings.data && {
    ...settings.data,
    project:
      projectId && ofProject.data?.project?.projectId === projectId ? ofProject.data.project : null,
  };
  const failure = settings.error ?? ofProject.error ?? tools.error;
  return (
    <section aria-labelledby="settings-mcp" className="flex flex-col gap-3">
      <h3 id="settings-mcp" className="font-semibold">
        {S.title}
      </h3>
      <div className="flex flex-wrap items-center gap-2">
        <span className={NOTE}>{S.intro}</span>
        {view && <OpenFile path={view.file} native={native} />}
      </div>
      {!view && !failure && <span className={NOTE}>{S.loading}</span>}
      {failure && (
        <span role="alert" className={ALERT}>
          {S.failed(failure instanceof Error ? failure.message : String(failure))}
        </span>
      )}
      {view && (
        <>
          {view.warnings.map((w) => (
            <span key={w} data-warning="" className={ALERT}>
              {S.warning(w)}
            </span>
          ))}
          <PresetRow view={view} />
          <AutoApproveRow view={view} />
          <RetentionRow view={view} />
          {view.http && <HttpRow http={view.http} native={native} />}
          <ProjectPicker
            projectId={projectId}
            onChange={setProjectId}
            view={view}
            native={native}
          />
          <GroupsRow view={view} />
        </>
      )}
      {tools.data &&
        (projectId ? (
          <ProjectTools projectId={projectId} global={tools.data} />
        ) : (
          <ToolList global={tools.data} project={undefined} />
        ))}
      <Registration native={native} />
    </section>
  );
}

/**
 * 그 프로젝트가 `[mcp]` 아래에 적은 키의 값과 파일(`groups.db` 처럼 점으로 이은 키). 같은 키를 여러 파일이 적었으면 병합에서
 * 이기는 마지막 것(Host 가 공유 → 개인 차례로 준다)을 보인다.
 */
function overrideOf(view: McpSettingsView, key: string) {
  return view.project?.overrides.filter((o) => o.key === key).at(-1);
}

function OverrideNote({ view, settingKey }: { view: McpSettingsView; settingKey: string }) {
  const S = useMcpStrings().settings;
  const o = overrideOf(view, settingKey);
  if (!o) return null;
  return (
    <span data-override={settingKey} className={NOTE}>
      {S.projectSets}: {JSON.stringify(o.value)} {S.overrideFrom(S.file[o.file])}
    </span>
  );
}

function PresetRow({ view }: { view: McpSettingsView }) {
  const S = useMcpStrings().settings;
  const { save, error, busy } = useSetting();
  return (
    <div data-setting="preset" className="flex flex-col gap-0.5">
      <label className="flex items-center gap-3">
        <span>{S.preset}</span>
        <select
          value={view.preset}
          disabled={busy}
          onChange={(e) => void save("preset", null, e.target.value)}
          className={SELECT}
        >
          {PRESETS.map((p) => (
            <option key={p} value={p}>
              {S.presets[p]}
            </option>
          ))}
        </select>
      </label>
      <OverrideNote view={view} settingKey="preset" />
      <ErrorLine error={error} />
    </div>
  );
}

function AutoApproveRow({ view }: { view: McpSettingsView }) {
  const S = useMcpStrings().settings;
  const tier = useMcpStrings().approval.tier;
  const { save, error, busy } = useSetting();
  return (
    <div data-setting="autoApprove" className="flex flex-col gap-0.5">
      <div className="flex flex-wrap items-center gap-3">
        <span>{S.autoApprove}</span>
        {AUTO_TIERS.map((t) => (
          <label key={t} className="flex items-center gap-1">
            <input
              type="checkbox"
              data-tier={t}
              checked={view.autoApprove.includes(t)}
              disabled={busy}
              onChange={(e) => {
                const next = e.target.checked
                  ? [...view.autoApprove, t]
                  : view.autoApprove.filter((x) => x !== t);
                void save("autoApprove", null, next);
              }}
            />
            {tier[t]}
          </label>
        ))}
      </div>
      <span className={NOTE}>{S.autoApproveNote}</span>
      <OverrideNote view={view} settingKey="auto_approve" />
      {view.project && (
        <span data-project-auto-approve="" className={NOTE}>
          {S.projectAutoApprove}:{" "}
          {view.project.autoApprove.length > 0
            ? view.project.autoApprove.map((t) => tier[t]).join(", ")
            : S.projectAutoApproveNone}
        </span>
      )}
      <ErrorLine error={error} />
    </div>
  );
}

function RetentionRow({ view }: { view: McpSettingsView }) {
  const S = useMcpStrings().settings;
  const { save, error, busy } = useSetting();
  const [text, setText] = useState(String(view.retentionDays));
  const [seen, setSeen] = useState(view.retentionDays);
  // Host 가 돌려준 값이 바뀌면(저장 · 기본값으로 · 파일을 손으로 고침) 칸을 그 값으로 둔다.
  if (seen !== view.retentionDays) {
    setSeen(view.retentionDays);
    setText(String(view.retentionDays));
  }
  return (
    <div data-setting="retentionDays" className="flex flex-col gap-0.5">
      <form
        className="flex items-center gap-2"
        onSubmit={(e) => {
          e.preventDefault();
          // 수로 읽히면 수, 아니면 글 그대로 — 판정은 Host 가 한다.
          const trimmed = text.trim();
          const n = Number(trimmed);
          void save("retentionDays", null, trimmed !== "" && Number.isFinite(n) ? n : trimmed);
        }}
      >
        <label htmlFor="mcp-retention">{S.retention}</label>
        <input
          id="mcp-retention"
          value={text}
          inputMode="numeric"
          onChange={(e) => setText(e.target.value)}
          aria-invalid={error !== undefined}
          className="w-20 rounded-sm border border-ui-border bg-ui-background px-1.5 py-0.5"
        />
        <span className={NOTE}>{S.retentionUnit}</span>
        <button type="submit" className={BUTTON} disabled={busy}>
          {S.apply}
        </button>
        <button
          type="button"
          className={BUTTON}
          disabled={busy}
          onClick={() => void save("retentionDays", null, null)}
        >
          {S.reset}
        </button>
      </form>
      <span className={NOTE}>
        {S.retentionHint(view.maxRetentionDays, view.defaultRetentionDays)}
      </span>
      <ErrorLine error={error} />
    </div>
  );
}

/** HTTP 클라이언트 설정 예(토큰 자리는 `<token>`). Claude Code: https://code.claude.com/docs/en/mcp 의 `--transport http` ·
 * `--header`. Cursor: https://cursor.com/docs/mcp 의 `url` · `headers`. */
export function httpClients(url: string) {
  return [
    {
      id: "claudeCode",
      text: `claude mcp add --transport http --scope user pitwall ${url} --header "Authorization: Bearer <token>"`,
    },
    {
      id: "cursor",
      text: JSON.stringify(
        { mcpServers: { pitwall: { url, headers: { Authorization: "Bearer <token>" } } } },
        null,
        2,
      ),
    },
  ] as const;
}

const MASK = "••••••••••••••••";

function HttpRow({ http, native }: { http: McpHttpStatus; native: SettingsNative }) {
  const S = useMcpStrings().settings;
  const H = S.http;
  const { client } = useHost();
  const { save, error, busy } = useSetting();
  const [port, setPort] = useState(http.port === null ? "" : String(http.port));
  const [seenPort, setSeenPort] = useState(http.port);
  if (seenPort !== http.port) {
    setSeenPort(http.port);
    setPort(http.port === null ? "" : String(http.port));
  }
  const [shown, setShown] = useState<string | null>(null);
  const [confirming, setConfirming] = useState(false);
  const [note, setNote] = useState<
    { kind: "copied" | "regenerated" } | { kind: "failed"; error: string }
  >();
  const fail = (e: unknown) =>
    setNote({ kind: "failed", error: e instanceof Error ? e.message : String(e) });
  const readToken = async (): Promise<string | null> => {
    if (!client) return null;
    const view = await client.call("mcp.http.token", null);
    return view.token;
  };
  return (
    <div data-setting="http" className="flex flex-col gap-1">
      <span className="font-semibold">{H.title}</span>
      <span className={NOTE}>{H.intro}</span>
      <label className="flex items-center gap-2">
        <input
          type="checkbox"
          data-http-enabled=""
          checked={http.enabled}
          disabled={busy}
          onChange={(e) => void save("httpEnabled", null, e.target.checked)}
        />
        {H.enabled}
      </label>
      <form
        className="flex items-center gap-2"
        onSubmit={(e) => {
          e.preventDefault();
          const trimmed = port.trim();
          const n = Number(trimmed);
          void save("httpPort", null, trimmed !== "" && Number.isFinite(n) ? n : trimmed);
        }}
      >
        <label htmlFor="mcp-http-port">{H.port}</label>
        <input
          id="mcp-http-port"
          value={port}
          inputMode="numeric"
          onChange={(e) => setPort(e.target.value)}
          aria-invalid={error !== undefined}
          className="w-24 rounded-sm border border-ui-border bg-ui-background px-1.5 py-0.5"
        />
        <button type="submit" className={BUTTON} disabled={busy}>
          {S.apply}
        </button>
        <button
          type="button"
          className={BUTTON}
          disabled={busy}
          onClick={() => void save("httpPort", null, null)}
        >
          {S.reset}
        </button>
      </form>
      <ErrorLine error={error} />
      <span data-http-state="" className={http.problem ? ALERT : NOTE}>
        {!http.enabled ? H.off : http.problem ? H.problem[http.problem] : H.listening}
      </span>
      {http.url && <CopyRow id="httpAddress" label={H.address} text={http.url} native={native} />}
      <div data-http-token="" className="flex flex-col gap-0.5">
        <div className="flex flex-wrap items-center gap-2">
          <span className="w-56 shrink-0">{H.token}</span>
          <code
            data-token-value=""
            className="flex-1 rounded-sm border border-ui-border px-1.5 py-0.5 font-mono text-[12px] break-all"
          >
            {shown ?? MASK}
          </code>
          <button
            type="button"
            className={BUTTON}
            onClick={() => {
              if (shown !== null) {
                setShown(null);
                return;
              }
              setNote(undefined);
              readToken().then((t) => setShown(t ?? H.tokenNone), fail);
            }}
          >
            {shown !== null ? H.hide : H.show}
          </button>
          <button
            type="button"
            className={BUTTON}
            onClick={() => {
              setNote(undefined);
              readToken()
                .then(async (t) => {
                  if (t === null) {
                    setShown(H.tokenNone);
                    return;
                  }
                  await native.copyText(t);
                  setNote({ kind: "copied" });
                })
                .catch(fail);
            }}
          >
            {S.copy}
          </button>
          {!confirming && (
            <button type="button" className={BUTTON} onClick={() => setConfirming(true)}>
              {H.regenerate}
            </button>
          )}
        </div>
        <span className={NOTE}>{H.tokenNote(http.tokenFile)}</span>
        {confirming && (
          <div data-http-regenerate="" className="flex flex-wrap items-center gap-2">
            <span role="alert" className={ALERT}>
              {H.regenerateConfirm}
            </span>
            <button
              type="button"
              className={BUTTON}
              data-confirm=""
              onClick={() => {
                setConfirming(false);
                setNote(undefined);
                if (!client) return;
                client.call("mcp.http.regenerateToken", null).then(() => {
                  // 보이던 옛 토큰은 버린다 — 새 토큰은 [보기] 로 다시 읽는다.
                  setShown(null);
                  setNote({ kind: "regenerated" });
                }, fail);
              }}
            >
              {H.regenerateYes}
            </button>
            <button type="button" className={BUTTON} onClick={() => setConfirming(false)}>
              {H.cancel}
            </button>
          </div>
        )}
        {note?.kind === "copied" && (
          <span role="status" className={NOTE}>
            {S.copied}
          </span>
        )}
        {note?.kind === "regenerated" && (
          <span role="status" className={NOTE}>
            {H.regenerated}
          </span>
        )}
        {note?.kind === "failed" && (
          <span role="alert" className={ALERT}>
            {H.tokenFailed(note.error)}
          </span>
        )}
      </div>
      {http.url && (
        <div data-http-clients="" className="flex flex-col gap-1">
          <span>{H.clients}</span>
          <span className={NOTE}>{H.clientsNote}</span>
          {httpClients(http.url).map((c) => (
            <CopyRow
              key={c.id}
              id={`http-${c.id}`}
              label={S.registerClient[c.id]}
              text={c.text}
              native={native}
            />
          ))}
        </div>
      )}
    </div>
  );
}

function ProjectPicker({
  projectId,
  onChange,
  view,
  native,
}: {
  projectId: string;
  onChange(id: string): void;
  view: McpSettingsView;
  native: SettingsNative;
}) {
  const S = useMcpStrings().settings;
  const projects = useRpcQuery("project.list", null);
  const project = view.project;
  return (
    <div data-setting="project" className="flex flex-col gap-1">
      <label className="flex items-center gap-3">
        <span>{S.project}</span>
        <select value={projectId} onChange={(e) => onChange(e.target.value)} className={SELECT}>
          <option value="">{S.projectNone}</option>
          {(projects.data ?? []).map((p: Project) => (
            <option key={p.id} value={p.id}>
              {p.name}
            </option>
          ))}
        </select>
      </label>
      {project && project.projectId === projectId && (
        <div data-project-overrides="" className="flex flex-col gap-1 pl-3">
          <span data-project-note="" className={NOTE}>
            {S.projectNote}
          </span>
          <span className={NOTE}>{S.projectFiles}</span>
          {project.files.map((f) => (
            <div key={f.path} data-file={f.kind} className="flex items-center gap-2">
              <span>{S.file[f.kind]}</span>
              {f.exists ? (
                <OpenFile path={f.path} native={native} />
              ) : (
                <span className={NOTE}>{S.fileMissing}</span>
              )}
            </div>
          ))}
          <span className={NOTE}>{S.projectOverrides}</span>
          {project.overrides.length === 0 ? (
            <span className={NOTE}>{S.projectNoOverrides}</span>
          ) : (
            <ul className="font-mono text-[12px]">
              {project.overrides.map((o) => (
                <li key={`${o.file}:${o.key}`} data-override-key={o.key}>
                  {o.key} = {JSON.stringify(o.value)}{" "}
                  <span className={NOTE}>{S.overrideFrom(S.file[o.file])}</span>
                </li>
              ))}
            </ul>
          )}
          {project.warnings.map((w) => (
            <span key={w} data-warning="" className={ALERT}>
              {S.warning(w)}
            </span>
          ))}
        </div>
      )}
    </div>
  );
}

function GroupsRow({ view }: { view: McpSettingsView }) {
  const S = useMcpStrings().settings;
  const { save, error, busy } = useSetting();
  return (
    <div data-setting="groups" className="flex flex-col gap-1">
      <span>{S.groups}</span>
      <div className="grid grid-cols-[max-content_max-content_1fr] items-center gap-x-3 gap-y-1">
        {view.groups.map((g) => (
          <div key={g.group} data-group={g.group} className="contents">
            <label htmlFor={`mcp-group-${g.group}`} className="font-mono">
              {g.group}
            </label>
            <select
              id={`mcp-group-${g.group}`}
              value={g.enabled === null ? "default" : g.enabled ? "on" : "off"}
              disabled={busy}
              onChange={(e) =>
                void save(
                  "group",
                  g.group,
                  e.target.value === "default" ? null : e.target.value === "on",
                )
              }
              className={SELECT}
            >
              <option value="default">{S.group.default}</option>
              <option value="on">{S.group.on}</option>
              <option value="off">{S.group.off}</option>
            </select>
            <OverrideNote view={view} settingKey={`groups.${g.group}`} />
          </div>
        ))}
      </div>
      <ErrorLine error={error} />
    </div>
  );
}

function ProjectTools({ projectId, global }: { projectId: string; global: McpToolInfo[] }) {
  const project = useRpcQuery("mcp.tools.list", { projectId }, { retry: false });
  return <ToolList global={global} project={project.data} />;
}

function ToolList({
  global,
  project,
}: {
  global: McpToolInfo[];
  /** 고른 프로젝트의 판정(이름 → 판정). 없으면 프로젝트를 고르지 않았거나 아직 읽는 중이다. */
  project: McpToolInfo[] | undefined;
}) {
  const S = useMcpStrings().settings;
  const tier = useMcpStrings().approval.tier;
  const [query, setQuery] = useState("");
  const [open, setOpen] = useState<string>();
  const { save, error, busy } = useSetting();
  const byName = useMemo(() => new Map(project?.map((t) => [t.name, t])), [project]);
  const shown = useMemo(
    () =>
      global
        .filter((t) => matches(t, query))
        .sort((a, b) => a.group.localeCompare(b.group) || a.name.localeCompare(b.name)),
    [global, query],
  );
  return (
    <div data-setting="tools" className="flex flex-col gap-1">
      <div className="flex items-center gap-3">
        <span>{S.tools}</span>
        <input
          type="search"
          aria-label={S.search}
          placeholder={S.search}
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          className="w-56 rounded-sm border border-ui-border bg-ui-background px-1.5 py-0.5"
        />
        <span className={NOTE}>{S.toolsCount(shown.length)}</span>
      </div>
      <ErrorLine error={error} />
      {shown.length === 0 ? (
        <span className={NOTE}>{S.noTools}</span>
      ) : (
        <table className="w-full text-left text-[12px]">
          <thead className="text-ui-muted-foreground">
            <tr>
              <th className="pr-2 font-normal">{S.column.on}</th>
              <th className="pr-2 font-normal">{S.column.name}</th>
              <th className="pr-2 font-normal">{S.column.tier}</th>
              <th className="pr-2 font-normal">{S.column.group}</th>
              <th className="pr-2 font-normal">{S.column.calls}</th>
              <th className="font-normal">{S.column.why}</th>
            </tr>
          </thead>
          <tbody>
            {shown.map((tool) => {
              return (
                <ToolRow
                  key={tool.name}
                  tool={tool}
                  theirs={byName.get(tool.name)}
                  tierText={tier[tool.tier]}
                  busy={busy}
                  open={open === tool.name}
                  onOpen={() => setOpen(open === tool.name ? undefined : tool.name)}
                  onToggle={(on) => void save("tool", tool.name, on)}
                  onReset={() => void save("tool", tool.name, null)}
                />
              );
            })}
          </tbody>
        </table>
      )}
    </div>
  );
}

function ToolRow({
  tool,
  theirs,
  tierText,
  busy,
  open,
  onOpen,
  onToggle,
  onReset,
}: {
  tool: McpToolInfo;
  /** 고른 프로젝트에서의 판정(프로젝트를 고르지 않았거나 아직 읽는 중이면 없다). 토글은 잠그지 않는다. */
  theirs: McpToolInfo | undefined;
  tierText: string;
  busy: boolean;
  open: boolean;
  onOpen(): void;
  onToggle(on: boolean): void;
  onReset(): void;
}) {
  const S = useMcpStrings().settings;
  const different = differs(tool, theirs);
  return (
    <>
      <tr data-tool={tool.name} data-differs={different ? "" : undefined} className="align-top">
        <td className="pr-2">
          <input
            type="checkbox"
            aria-label={tool.name}
            checked={tool.enabled}
            disabled={busy}
            onChange={(e) => onToggle(e.target.checked)}
          />
        </td>
        <td className="pr-2">
          <div className="font-mono">{tool.name}</div>
          <div className={NOTE}>{tool.description}</div>
          <button type="button" className="text-[11px] underline" onClick={onOpen}>
            {open ? S.schemaHide : S.schemaShow}
          </button>
        </td>
        <td className="pr-2">{tierText}</td>
        <td className="pr-2 font-mono">{tool.group}</td>
        <td className="pr-2 tabular-nums">{tool.recentCalls}</td>
        <td>
          <div data-why="">{ruleText(tool, S)}</div>
          {tool.rule === "tool" && (
            <button type="button" className={BUTTON} disabled={busy} onClick={onReset}>
              {S.reset}
            </button>
          )}
          {theirs && (
            <div data-project-verdict="" className={different ? ALERT : NOTE}>
              {different && (
                <span className="mr-1 rounded-sm border border-ui-border px-1">
                  {S.differsBadge}
                </span>
              )}
              {S.inProject(theirs.enabled ? S.stateOn : S.stateOff, ruleText(theirs, S))}
            </div>
          )}
        </td>
      </tr>
      {open && (
        <tr data-schema={tool.name}>
          <td />
          <td colSpan={5}>
            <pre className="max-h-64 overflow-auto rounded-sm border border-ui-border p-2 font-mono text-[11px]">
              {JSON.stringify(tool.inputSchema, null, 2)}
            </pre>
          </td>
        </tr>
      )}
    </>
  );
}

function Registration({ native }: { native: SettingsNative }) {
  const S = useMcpStrings().settings;
  return (
    <div data-setting="register" className="flex flex-col gap-1">
      <span>{S.register}</span>
      <span className={NOTE}>{S.registerIntro}</span>
      {REGISTRATIONS.map((r) => (
        <CopyRow
          key={r.id}
          id={r.id}
          label={S.registerClient[r.id]}
          text={r.text}
          native={native}
        />
      ))}
    </div>
  );
}

function CopyRow({
  id,
  label,
  text,
  native,
}: {
  id: string;
  label: string;
  text: string;
  native: SettingsNative;
}) {
  const S = useMcpStrings().settings;
  const [state, setState] = useState<
    { kind: "idle" | "copied" } | { kind: "failed"; error: string }
  >({ kind: "idle" });
  return (
    <div data-register={id} className="flex flex-col gap-0.5">
      <div className="flex items-center gap-2">
        <span className="w-56 shrink-0">{label}</span>
        <code className="flex-1 rounded-sm border border-ui-border px-1.5 py-0.5 font-mono text-[12px] whitespace-pre-wrap">
          {text}
        </code>
        <button
          type="button"
          className={BUTTON}
          onClick={() =>
            native.copyText(text).then(
              () => setState({ kind: "copied" }),
              (e: unknown) =>
                setState({ kind: "failed", error: e instanceof Error ? e.message : String(e) }),
            )
          }
        >
          {S.copy}
        </button>
      </div>
      {state.kind === "copied" && (
        <span role="status" className={NOTE}>
          {S.copied}
        </span>
      )}
      {state.kind === "failed" && (
        <span role="alert" className={ALERT}>
          {S.copyFailed(state.error)}
        </span>
      )}
    </div>
  );
}
