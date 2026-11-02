// 설정 창 「도구」(07 §4.4 「관리 화면」 · 10-ui S7 · 15-m4-plan M4-17): 설치된 도구 표(이름 · 판 · 언어 · 분류 · 라이선스 · 크기 ·
// 체크섬 유무 · 쓰는 서버), registry 검색(분류 · 언어 거르기) → 설치 · 업데이트 · 이전 판으로 · 제거, 설치 로그, registry 판 · 마지막
// 확인 · 「최신 registry 사용」(결정 M4-D16 — `[tools] registry`).
//
// 설치 · 업데이트는 확인 흐름(install.ts)을 지난다 — 설치기가 `4001` 이면 라이선스 · 출처 다이얼로그(처음 포커스 [취소])를 띄우고
// [설치] 를 누른 때만 그 값을 실어 다시 부른다. registry 의 글(설명 · 라이선스 · 주소)은 글로만 그리고 링크는 http(s) 만이다.
// 설치는 Host 가 판정한다(설정 창 · 프로젝트 창만, tier exec — M4-04) — 이 쪽은 그 답을 보일 뿐이다.

import type {
  ToolInstalledInfo,
  ToolLog,
  ToolPackageSummary,
  ToolsList,
  ToolsRegistryStatus,
  ToolsUpdates,
  LspServerPackage,
} from "@pitwall/rpc-client";
import { RpcCallError } from "@pitwall/rpc-client";
import { openUrl } from "@tauri-apps/plugin-opener";
import * as Dialog from "@radix-ui/react-dialog";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { useKeymapPopup } from "../keymap/keymap-provider";
import { useHost } from "../rpc/host";
import { useToolConfirm } from "./confirm-dialog";
import { changeTool, formatBytes, TOOLS_ERROR, type ToolsCall } from "./install";
import { type ToolsStrings, useToolsStrings } from "./strings";

const BUTTON =
  "rounded-sm border border-ui-border bg-ui-panel px-2 py-0.5 hover:bg-ui-hover disabled:opacity-50";
const NOTE = "text-[12px] text-ui-muted-foreground";
/** registry 의 분류(Mason `categories`). */
const CATEGORIES = ["LSP", "Formatter", "Linter", "DAP"];

function errorText(e: unknown, T: ToolsStrings["manage"]): string {
  if (e instanceof RpcCallError && e.code === TOOLS_ERROR.RUNTIME_MISSING) {
    const runtime = (e.data as { runtime?: string } | undefined)?.runtime;
    if (runtime) return T.runtimeMissing(runtime);
  }
  if (e instanceof RpcCallError && typeof e.data === "string") return `${e.message}: ${e.data}`;
  return e instanceof Error ? e.message : String(e);
}

interface Data {
  list?: ToolsList;
  updates?: ToolsUpdates;
  registry?: ToolsRegistryStatus;
  packages?: readonly LspServerPackage[];
  /** 설치본의 registry 요약(언어 · 분류). */
  summaries: Record<string, ToolPackageSummary | undefined>;
}

function LogDialog({ name, log, onClose }: { name: string; log: ToolLog; onClose(): void }) {
  const T = useToolsStrings().manage;
  useKeymapPopup(true);
  return (
    <Dialog.Root open onOpenChange={(open) => !open && onClose()}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 bg-black/20" />
        <Dialog.Content
          data-testid="tools-log-dialog"
          aria-describedby={undefined}
          className="fixed top-1/2 left-1/2 flex h-[70vh] w-[760px] max-w-[94vw] -translate-x-1/2 -translate-y-1/2 flex-col gap-2 rounded-md border border-ui-border bg-ui-panel px-3 py-2 text-ui-foreground shadow-lg"
        >
          <Dialog.Title className="text-sm font-semibold">{`${T.log} — ${name}`}</Dialog.Title>
          <pre className="min-h-0 flex-1 overflow-auto rounded-sm border border-ui-border p-2 font-mono text-[12px] whitespace-pre-wrap">
            {log.text || T.logEmpty}
          </pre>
          <div className="flex justify-end">
            <button type="button" className={BUTTON} onClick={onClose}>
              {T.logClose}
            </button>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

function checkedText(status: ToolsRegistryStatus, T: ToolsStrings["manage"]): string {
  if (status.fetchedAtMs === null) return T.registrySnapshot;
  return T.registryChecked(new Date(Number(status.fetchedAtMs)).toLocaleString());
}

export function ToolsSection() {
  const S = useToolsStrings();
  const T = S.manage;
  const { client } = useHost();
  const call = useMemo<ToolsCall | undefined>(
    () => (client ? (method, params) => client.call(method, params) : undefined),
    [client],
  );
  const open = useCallback((url: string) => void openUrl(url).catch(() => {}), []);
  const confirm = useToolConfirm(call, open);
  const [data, setData] = useState<Data>({ summaries: {} });
  const [loadError, setLoadError] = useState<string>();
  const [busy, setBusy] = useState<string>();
  const [result, setResult] = useState<{ ok: boolean; text: string }>();
  const [log, setLog] = useState<{ name: string; log: ToolLog }>();
  const [query, setQuery] = useState("");
  const [category, setCategory] = useState("");
  const [language, setLanguage] = useState("");
  const [found, setFound] = useState<{ items: ToolPackageSummary[]; truncated: boolean }>();

  const reload = useCallback(async () => {
    if (!call) return;
    try {
      const [list, updates, registry, packages] = await Promise.all([
        call("tools.list", null),
        call("tools.updates", null),
        call("tools.registry.status", null),
        call("lsp.servers.packages", null).catch(() => [] as LspServerPackage[]),
      ]);
      const summaries: Data["summaries"] = {};
      await Promise.all(
        list.items.map(async (item) => {
          summaries[item.name] = await call("tools.registry.get", { name: item.name }).then(
            (d) => d.summary,
            () => undefined,
          );
        }),
      );
      setData({ list, updates, registry, packages, summaries });
      setLoadError(undefined);
    } catch (e) {
      setLoadError(errorText(e, T));
    }
  }, [call, T]);
  useEffect(() => void reload(), [reload]);

  // 검색(입력이 멈춘 뒤 한 번).
  const searching = useRef(0);
  useEffect(() => {
    if (!call) return;
    const id = ++searching.current;
    const timer = setTimeout(() => {
      call("tools.registry.search", {
        query,
        category: category || null,
        language: language || null,
        limit: 50,
      }).then(
        (r) => id === searching.current && setFound(r),
        () => {},
      );
    }, 150);
    return () => clearTimeout(timer);
  }, [call, query, category, language]);

  const run = async (name: string, work: () => Promise<string | undefined>) => {
    setBusy(name);
    setResult(undefined);
    try {
      const done = await work();
      if (done) setResult({ ok: true, text: done });
    } catch (e) {
      setResult({ ok: false, text: T.failed(name, errorText(e, T)) });
    } finally {
      setBusy(undefined);
      await reload();
    }
  };
  const change = (kind: "install" | "update", name: string) =>
    run(name, async () => {
      if (!call) return undefined;
      const done = await changeTool(call, confirm.ask, kind, name);
      return done ? T.done(done.name, done.version) : undefined;
    });
  const rollback = (name: string) =>
    run(name, async () => {
      const info = await call!("tools.rollback", { name });
      return T.done(info.name, info.version);
    });
  const uninstall = (name: string) =>
    run(name, async () => {
      await call!("tools.uninstall", { name });
      return undefined;
    });
  const showLog = async (name: string) => {
    if (!call) return;
    try {
      setLog({ name, log: await call("tools.log", { name }) });
    } catch (e) {
      setResult({ ok: false, text: T.failed(name, errorText(e, T)) });
    }
  };
  const setLatest = async (latest: boolean) => {
    if (!call) return;
    try {
      const registry = await call("tools.registry.setMode", { mode: latest ? "latest" : "pinned" });
      setData((d) => ({ ...d, registry }));
    } catch (e) {
      setResult({ ok: false, text: T.failed(T.registry, errorText(e, T)) });
    }
  };

  const installed = new Map((data.list?.items ?? []).map((i) => [i.name, i]));
  const update = new Map((data.updates?.items ?? []).map((u) => [u.name, u]));
  const users = (name: string) =>
    (data.packages ?? [])
      .filter((p) => p.mason === name)
      .map((p) => p.server)
      .join(", ");
  const languages = useMemo(
    () => [...new Set((found?.items ?? []).flatMap((i) => i.languages))].sort(),
    [found],
  );

  return (
    <section
      aria-labelledby="settings-tools"
      className="flex flex-col gap-2"
      data-testid="tools-section"
    >
      <h3 id="settings-tools" className="font-semibold">
        {T.title}
      </h3>
      <p className={NOTE}>{T.intro}</p>
      {loadError && (
        <span role="alert" className="text-[12px] text-ui-accent">
          {T.loadFailed(loadError)}
        </span>
      )}
      {data.registry && (
        <div data-testid="tools-registry" className="flex flex-col gap-1">
          <span>
            {T.registry}: {T.registryVersion(data.registry.version, data.registry.pinnedVersion)}
          </span>
          <span className={NOTE}>{checkedText(data.registry, T)}</span>
          {data.registry.fetching && <span className={NOTE}>{T.registryFetching}</span>}
          {data.registry.lastError && (
            <span className="text-[12px] text-ui-accent">
              {T.registryError(data.registry.lastError)}
            </span>
          )}
          <label className="flex items-center gap-2">
            <input
              type="checkbox"
              data-testid="tools-registry-latest"
              checked={data.registry.mode === "latest"}
              onChange={(e) => void setLatest(e.target.checked)}
            />
            {T.registryLatest}
          </label>
        </div>
      )}
      <div role="status" aria-live="polite" className="text-[12px]">
        {busy && <span className={NOTE}>{T.busy(busy)}</span>}
        {result && (
          <span
            data-testid="tools-result"
            role={result.ok ? undefined : "alert"}
            className={result.ok ? undefined : "text-ui-accent"}
          >
            {result.text}
          </span>
        )}
      </div>
      <h4 className="font-semibold">{T.installed}</h4>
      {!data.list && !loadError && <span className={NOTE}>{T.loading}</span>}
      {data.list && data.list.items.length === 0 && data.list.unreadable.length === 0 && (
        <span className={NOTE}>{T.installedEmpty}</span>
      )}
      {data.list && data.list.items.length > 0 && (
        <table data-testid="tools-installed" className="w-full text-left text-[12px]">
          <thead className="text-ui-muted-foreground">
            <tr>
              <th>{T.columns.name}</th>
              <th>{T.columns.version}</th>
              <th>{T.columns.language}</th>
              <th>{T.columns.category}</th>
              <th>{T.columns.license}</th>
              <th>{T.columns.size}</th>
              <th>{T.columns.checksum}</th>
              <th>{T.columns.servers}</th>
              <th>{T.columns.actions}</th>
            </tr>
          </thead>
          <tbody>
            {data.list.items.map((item: ToolInstalledInfo) => {
              const summary = data.summaries[item.name];
              const next = update.get(item.name);
              return (
                <tr
                  key={item.name}
                  data-testid="tools-row"
                  data-name={item.name}
                  className="align-top"
                >
                  <td>
                    {item.name}
                    {item.problem && (
                      <div className="text-ui-accent">{T.problem(item.problem)}</div>
                    )}
                  </td>
                  <td>
                    {item.version}
                    {item.previous && <div className={NOTE}>{T.previous(item.previous)}</div>}
                  </td>
                  <td>{summary?.languages.join(", ") ?? ""}</td>
                  <td>{summary?.categories.join(", ") ?? ""}</td>
                  <td data-testid="tools-license">{item.licenses.join(" · ")}</td>
                  <td data-testid="tools-size">{formatBytes(Number(item.size))}</td>
                  <td
                    data-testid="tools-checksum"
                    className={item.checksum === "none" ? "text-ui-accent" : undefined}
                  >
                    {item.checksum === "pinned" ? T.checksumPinned : T.checksumNone}
                  </td>
                  <td>{users(item.name)}</td>
                  <td className="flex flex-wrap gap-1">
                    {next && (
                      <button
                        type="button"
                        data-testid="tools-update"
                        className={BUTTON}
                        disabled={busy !== undefined}
                        onClick={() => void change("update", item.name)}
                      >
                        {T.updateTo(next.available)}
                      </button>
                    )}
                    {item.previous && (
                      <button
                        type="button"
                        className={BUTTON}
                        disabled={busy !== undefined}
                        onClick={() => void rollback(item.name)}
                      >
                        {T.rollback}
                      </button>
                    )}
                    <button
                      type="button"
                      className={BUTTON}
                      disabled={busy !== undefined}
                      onClick={() => void uninstall(item.name)}
                    >
                      {T.uninstall}
                    </button>
                    <button
                      type="button"
                      className={BUTTON}
                      onClick={() => void showLog(item.name)}
                    >
                      {T.log}
                    </button>
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      )}
      {data.list?.unreadable.map((b) => (
        <span key={b.name} className="text-[12px] text-ui-accent">
          {b.name}: {T.problem(b.problem)}
        </span>
      ))}
      <h4 className="mt-2 font-semibold">{T.search}</h4>
      <div className="flex flex-wrap items-center gap-2">
        <input
          type="search"
          data-testid="tools-search"
          aria-label={T.search}
          placeholder={T.searchPlaceholder}
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          className="rounded-sm border border-ui-border bg-ui-background px-1.5 py-0.5"
        />
        <label className="flex items-center gap-1">
          {T.filterCategory}
          <select
            data-testid="tools-filter-category"
            value={category}
            onChange={(e) => setCategory(e.target.value)}
            className="rounded-sm border border-ui-border bg-ui-background px-1.5 py-0.5"
          >
            <option value="">{T.filterAll}</option>
            {CATEGORIES.map((c) => (
              <option key={c} value={c}>
                {c}
              </option>
            ))}
          </select>
        </label>
        <label className="flex items-center gap-1">
          {T.filterLanguage}
          <select
            data-testid="tools-filter-language"
            value={language}
            onChange={(e) => setLanguage(e.target.value)}
            className="rounded-sm border border-ui-border bg-ui-background px-1.5 py-0.5"
          >
            <option value="">{T.filterAll}</option>
            {[...new Set([...languages, ...(language ? [language] : [])])].map((l) => (
              <option key={l} value={l}>
                {l}
              </option>
            ))}
          </select>
        </label>
      </div>
      {found && found.items.length === 0 && <span className={NOTE}>{T.searchEmpty}</span>}
      {found && found.truncated && <span className={NOTE}>{T.truncated(found.items.length)}</span>}
      <ul data-testid="tools-results" className="flex flex-col gap-1">
        {found?.items.map((p) => (
          <li
            key={p.name}
            data-testid="tools-result-row"
            data-name={p.name}
            className="flex items-start gap-2 text-[12px]"
          >
            <div className="min-w-0 flex-1">
              <div>
                <span className="font-semibold">{p.name}</span> {p.version}
                {p.bundled && <span className={`${NOTE} ml-1`}>{T.bundled}</span>}
                {p.deprecated && <span className="ml-1 text-ui-accent">{T.deprecated}</span>}
              </div>
              <div className={NOTE}>
                {[p.licenses.join(" · "), p.languages.join(", "), p.categories.join(", ")]
                  .filter(Boolean)
                  .join(" — ")}
              </div>
              <div className="whitespace-pre-wrap">{p.description}</div>
              {p.unsupported && (
                <div className="text-ui-accent">{T.unsupported(p.unsupported.reason)}</div>
              )}
            </div>
            {installed.has(p.name) ? (
              <span className={NOTE}>{T.installedBadge}</span>
            ) : (
              <button
                type="button"
                data-testid="tools-install"
                className={BUTTON}
                disabled={busy !== undefined || p.unsupported !== null}
                onClick={() => void change("install", p.name)}
              >
                {T.install}
              </button>
            )}
          </li>
        ))}
      </ul>
      {log && <LogDialog name={log.name} log={log.log} onClose={() => setLog(undefined)} />}
      {confirm.element}
    </section>
  );
}
