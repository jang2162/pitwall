// Redis 키 문서(14-m3-plan M3-18, 10-ui.md 「에디터 영역 문서 타입」 의 Redis 키): 위 막대(연결 표지 · DB 번호 · 연결 · 키 패턴 ·
// 구분자 · 다시 찾기), 왼쪽 키 트리(key-tree-view.tsx), 오른쪽 값 뷰어(value-view.tsx), 아래 명령 콘솔(kv-console.tsx).
//
// - **연결은 사람이 [연결] 을 누를 때만** 연다(결정 M3-D13). 마운트(새로 열기 · 복원)는 연결 설정을 읽기만 한다(`db.connection.get`
//   — 이름 · 색 · 프로덕션 표지를 연결 전에도 보이려고. 설정 읽기는 연결을 열지 않는다, 결정 M3-D18). 세션은 M3-05 의
//   `db.session.open`(`database` = DB 번호, `tabId` = 탭의 것)이고, 상태바 `db` 항목에 `useDbTabSession` 으로 둔다(M3-19 ⓓ).
//   탭이 닫히면(언마운트) 세션을 닫는다.
// - **DB 번호 전환**은 그 번호의 연결로 옮긴다: 지금 세션을 닫고 탭의 `db.database` · 이름을 바꾼 뒤, 연결돼 있었으면 새 번호로 다시
//   연다(DB 번호마다 연결 하나 — 결정 M3-D15, `SELECT` 는 분류가 막는다).
// - 키 동작(TTL 수정 · 지우기, 삭제, 이름 바꾸기)은 전용 메서드(`db.kv.setTtl` · `persist` · `delete` · `rename`)다. 등급은 Host 가
//   정한다 — 화면은 미리 판정하지 않고 받은 `3001` 을 M3-19 다이얼로그(`KvConfirmQuestion`)로 묻는다. 이름 바꾸기는 먼저 덮지 않고
//   (`RENAMENX` — 그것도 확인이다), 대상이 있으면(`targetExists`) 「덮어쓴다」 안내와 함께 덮는 이름 바꾸기(`RENAME`)를 다시 묻는다.

import type {
  DbConfirmed,
  DbConnectionConfig,
  DbKvConfirmRequired,
  DbSessionOpened,
} from "@pitwall/rpc-client";
import { RotateCw, Square } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState, useSyncExternalStore } from "react";
import { useStore } from "zustand";

import { tabParams } from "../../editor/editor-tabs";
import { useEditorWorkspace } from "../../editor/editor-workspace";
import { useNotify } from "../../notifications/notifications";
import { useHost } from "../../rpc/host";
import type { DbDocumentProps } from "../documents/db-document-panel";
import { writeClipboardText } from "../grid/clipboard";
import { ConnectionMark, connectionFrameStyle, TlsMark } from "../safety/connection-mark";
import { useDbAccessPrompt } from "../safety/access-prompt";
import { type DbActiveSession, useDbTabSession } from "../safety/db-status";
import { DbRefusalMessage, dbRefusal, dbRefusalText } from "../safety/messages";
import { type KvConfirmQuestion, useRiskConfirm } from "../safety/risk-confirm";
import { useDbSafetyStrings } from "../safety/strings";
import { KeyPromptDialog, type KeyPromptQuestion } from "./key-prompt";
import { KeyScan, type ScanState } from "./key-scan";
import { DEFAULT_SEPARATOR, type KeyEntry, KeyTree, keyName, sameKey } from "./key-tree";
import { KeyTreeView } from "./key-tree-view";
import { errorText, withKvConfirm } from "./kv-gate";
import { KvConsole, type KvConsoleSession } from "./kv-console";
import { redisProject } from "./redis-feature";
import { useDbRedisStrings } from "./strings";
import { ValueView } from "./value-view";

/** 위 막대의 DB 번호 고르기에 늘 보이는 번호(Redis 기본 `databases 16`). 지금 번호가 더 크면 그것도 넣는다. */
const DATABASES = 16;
/** TTL 칸이 받는 초의 자릿수(밀리초로 바꿔도 2^53 안). */
const MAX_TTL_DIGITS = 12;

function useScanState(scan: KeyScan): ScanState {
  return useSyncExternalStore(
    useCallback((f) => scan.subscribe(f), [scan]),
    () => scan.state,
  );
}

export function RedisDocument({ spec, params, panelId }: DbDocumentProps<"redis">) {
  const T = useDbRedisStrings();
  const S = useDbSafetyStrings();
  const { client } = useHost();
  const projectId = useStore(redisProject, (s) => s.projectId);
  const workspace = useEditorWorkspace();
  const notify = useNotify();
  const { confirm, element: confirmElement } = useRiskConfirm();
  const access = useDbAccessPrompt();
  const worktree = params.worktree ?? null;

  // 연결 설정(읽기만). 못 읽으면 `null` — 표지는 연결 id 로.
  const [config, setConfig] = useState<DbConnectionConfig | null>(null);
  useEffect(() => {
    if (!client || !projectId) return;
    let live = true;
    client.call("db.connection.get", { projectId, worktree, connectionId: spec.connectionId }).then(
      (entry) => live && setConfig(entry.config),
      () => live && setConfig(null),
    );
    return () => {
      live = false;
    };
  }, [client, projectId, worktree, spec.connectionId]);

  const [opened, setOpened] = useState<DbSessionOpened>();
  const [connecting, setConnecting] = useState(false);
  const [connectError, setConnectError] = useState<unknown>();
  const name = opened?.name ?? config?.name ?? spec.connectionId;
  const production = opened?.production ?? config?.production ?? false;
  const readOnly = opened?.readOnly ?? config?.readOnly ?? false;
  const color = config?.color ?? null;
  const mark = { name, production, readOnly, color };
  const active = useMemo(
    (): DbActiveSession | undefined =>
      opened && {
        session: opened.session,
        name: opened.name,
        production: opened.production,
        readOnly: opened.readOnly,
        color,
      },
    [opened, color],
  );
  useDbTabSession(panelId, active);

  const [separator, setSeparator] = useState(DEFAULT_SEPARATOR);
  const [pattern, setPattern] = useState("");
  const [browse, setBrowse] = useState(() => {
    const tree = new KeyTree(DEFAULT_SEPARATOR);
    return { tree, scan: new KeyScan(tree) };
  });
  const scanState = useScanState(browse.scan);
  // 트리에서 고른 키(줄 칠하기)와 값 뷰어가 보이는 키는 따로다 — 고르기만으로 값을 읽지 않는다(10-ui.md §5.6: 더블클릭 · Enter 가
  // 값 보기). ↑ ↓ 로 훑을 때마다 읽고 버리면 Host 가 버린 요청의 연결을 매번 다시 만든다(M3-22 ⓓ).
  const [selected, setSelected] = useState<KeyEntry>();
  const [viewing, setViewing] = useState<KeyEntry>();
  const [valueRound, setValueRound] = useState(0);

  // 탭이 닫히면 세션을 닫는다(최근 값을 본다).
  const sessionRef = useRef<string | undefined>(undefined);
  sessionRef.current = opened?.session.sessionId;
  const clientRef = useRef(client);
  clientRef.current = client;
  const scanRef = useRef(browse.scan);
  scanRef.current = browse.scan;
  // 탭이 닫힌 뒤에 풀린 `db.session.open` 은 그 세션을 곧바로 닫고 스캔을 시작하지 않는다(정리는 이미 지나갔다).
  const mountedRef = useRef(true);
  useEffect(
    () => () => {
      mountedRef.current = false;
      scanRef.current.stop();
      const sessionId = sessionRef.current;
      if (sessionId)
        void clientRef.current?.call("db.session.close", { sessionId }).catch(() => {});
    },
    [],
  );

  const startScan = useCallback(
    (sessionId: string, keyPattern: string) => {
      if (!client) return;
      browse.scan.stop();
      const tree = new KeyTree(separator);
      const scan = new KeyScan(tree);
      setBrowse({ tree, scan });
      setSelected(undefined);
      setViewing(undefined);
      void scan.run(client, { sessionId, pattern: keyPattern === "" ? null : keyPattern });
    },
    [client, browse.scan, separator],
  );

  const connect = async (database = spec.database) => {
    if (!client || !projectId || connecting) return;
    setConnecting(true);
    setConnectError(undefined);
    const open = () =>
      client.call("db.session.open", {
        projectId,
        worktree,
        connectionId: spec.connectionId,
        database,
        tabId: spec.tabId,
      });
    try {
      let o: DbSessionOpened;
      try {
        o = await open();
      } catch (e) {
        // 비밀(3101) · 확인(3102)이 필요하면 M3-13 의 공유 묻기로 받아 넣고 한 번 다시 연다.
        const target = {
          calls: client,
          projectId,
          worktree,
          connectionId: spec.connectionId,
          connectionName: name,
        };
        if (!(await access.resolve(e, target))) throw e;
        if (!mountedRef.current) return;
        o = await open();
      }
      if (!mountedRef.current) {
        void client.call("db.session.close", { sessionId: o.session.sessionId }).catch(() => {});
        return;
      }
      setOpened(o);
      startScan(o.session.sessionId, pattern);
    } catch (e) {
      setConnectError(e);
    } finally {
      setConnecting(false);
    }
  };

  const disconnect = () => {
    browse.scan.stop();
    const sessionId = opened?.session.sessionId;
    if (sessionId) void client?.call("db.session.close", { sessionId }).catch(() => {});
    setOpened(undefined);
    setSelected(undefined);
    setViewing(undefined);
  };

  const switchDatabase = (database: string) => {
    if (database === spec.database) return;
    const panel = workspace.api()?.getPanel(panelId);
    if (!panel) return;
    const wasConnected = opened !== undefined;
    disconnect();
    panel.api.updateParameters({ ...tabParams(panel), db: { ...spec, database } });
    panel.api.setTitle(T.title(name, database));
    const tree = new KeyTree(separator);
    setBrowse({ tree, scan: new KeyScan(tree) });
    if (wasConnected) void connect(database);
  };

  const sessionId = opened?.session.sessionId;
  const { tree, scan } = browse;

  // 확인 · 입력 다이얼로그.
  const ask = useCallback(
    (note?: string) => (kv: DbKvConfirmRequired) => {
      const question: KvConfirmQuestion = {
        kv,
        connection: { name, production, readOnly, color },
        ...(note !== undefined && { note }),
      };
      return confirm(question);
    },
    [confirm, name, production, readOnly, color],
  );
  const [prompt, setPrompt] = useState<{
    question: KeyPromptQuestion;
    answer(value: string | undefined): void;
  }>();
  const askValue = (question: KeyPromptQuestion) =>
    new Promise<string | undefined>((resolve) =>
      setPrompt({
        question,
        answer: (value) => {
          setPrompt(undefined);
          resolve(value);
        },
      }),
    );

  const fail = (e: unknown) => {
    const refusal = dbRefusal(e);
    notify({
      level: "error",
      title: T.key.failed(refusal ? dbRefusalText(refusal, S.refusal) : errorText(e)),
    });
  };
  const done = (title: string) => notify({ level: "info", title });
  const changedValue = () => setValueRound((r) => r + 1);

  const rename = async (entry: KeyEntry) => {
    if (!client || !sessionId) return;
    const to = await askValue({
      title: T.key.renameTitle(entry.name),
      label: T.key.renameTo,
      initial: entry.name,
      apply: T.key.apply,
      cancel: T.key.cancel,
      check: (v) => (v === "" || v === entry.name ? T.key.renameSame : undefined),
    });
    if (to === undefined) return;
    const call = (overwrite: boolean) => (confirmed: DbConfirmed | undefined) =>
      client.call("db.kv.rename", {
        sessionId,
        from: entry.key,
        to,
        overwrite,
        confirmed: confirmed ?? null,
      });
    try {
      let result = await withKvConfirm(call(false), ask());
      if (result.kind === "done" && result.value === "targetExists")
        result = await withKvConfirm(call(true), ask(T.key.overwriteNote(to)));
      if (result.kind !== "done" || result.value !== "renamed") return;
      const info = entry.info;
      tree.remove(entry.key);
      tree.remove(to);
      tree.add([{ ...info, key: to }]);
      setSelected(tree.find(to));
      setViewing(tree.find(to));
      done(T.key.renamed(entry.name, to));
    } catch (e) {
      fail(e);
    }
  };

  const keyActions = {
    open: (entry: KeyEntry) => {
      setSelected(entry);
      setViewing(entry);
      changedValue();
    },
    editTtl: async (entry: KeyEntry) => {
      if (!client || !sessionId) return;
      const left = entry.info.ttlMs;
      const value = await askValue({
        title: T.key.ttlTitle(entry.name),
        label: T.key.ttlSeconds,
        initial: left === null ? "" : String(Math.max(1, Math.ceil(left / 1000))),
        apply: T.key.apply,
        cancel: T.key.cancel,
        check: (v) =>
          new RegExp(`^\\d{1,${MAX_TTL_DIGITS}}$`).test(v.trim()) && Number(v.trim()) >= 1
            ? undefined
            : T.key.ttlInvalid,
      });
      if (value === undefined) return;
      const ttlMs = Number(value.trim()) * 1000;
      try {
        const result = await withKvConfirm(
          (confirmed) =>
            client.call("db.kv.setTtl", {
              sessionId,
              key: entry.key,
              ttlMs,
              confirmed: confirmed ?? null,
            }),
          ask(),
        );
        if (result.kind !== "done") return;
        if (result.value) {
          tree.setTtl(entry.key, ttlMs);
          done(T.key.ttlChanged(entry.name));
        } else done(T.key.gone);
        changedValue();
      } catch (e) {
        fail(e);
      }
    },
    clearTtl: async (entry: KeyEntry) => {
      if (!client || !sessionId) return;
      try {
        const result = await withKvConfirm(
          (confirmed) =>
            client.call("db.kv.persist", {
              sessionId,
              key: entry.key,
              confirmed: confirmed ?? null,
            }),
          ask(),
        );
        if (result.kind !== "done") return;
        if (result.value) {
          tree.setTtl(entry.key, null);
          done(T.key.ttlCleared(entry.name));
        }
        changedValue();
      } catch (e) {
        fail(e);
      }
    },
    remove: async (entry: KeyEntry) => {
      if (!client || !sessionId) return;
      try {
        const result = await withKvConfirm(
          (confirmed) =>
            client.call("db.kv.delete", {
              sessionId,
              keys: [entry.key],
              confirmed: confirmed ?? null,
            }),
          ask(),
        );
        if (result.kind !== "done") return;
        tree.remove(entry.key);
        const other = (s: KeyEntry | undefined) => (s && sameKey(s.key, entry.key) ? undefined : s);
        setSelected(other);
        setViewing(other);
        done(result.value.deleted > 0 ? T.key.deleted(entry.name) : T.key.gone);
      } catch (e) {
        fail(e);
      }
    },
    rename: (entry: KeyEntry) => void rename(entry),
    copyName: (entry: KeyEntry) =>
      void writeClipboardText(keyName(entry.key)).then(
        () => done(T.key.copied),
        (e: unknown) => fail(e),
      ),
  };

  const consoleSession = useMemo(
    (): KvConsoleSession | undefined =>
      client && sessionId
        ? { client, sessionId, ask: ask(), onWrite: () => setValueRound((r) => r + 1) }
        : undefined,
    [client, sessionId, ask],
  );

  const scanText = (() => {
    switch (scanState.kind) {
      case "idle":
        return "";
      case "scanning":
        return T.tree.scanning(tree.size);
      case "done":
        return T.tree.scanned(tree.size, scanState.elapsedMs);
      case "stopped":
        return T.tree.stopped(tree.size);
      case "capped":
        return T.tree.capped(tree.size);
      case "failed": {
        const refusal = dbRefusal(scanState.error);
        return T.tree.failed(
          refusal ? dbRefusalText(refusal, S.refusal) : errorText(scanState.error),
        );
      }
    }
  })();
  const databases = useMemo(() => {
    const n = Number(spec.database);
    return Array.from({ length: Math.max(DATABASES, n + 1) }, (_, i) => String(i));
  }, [spec.database]);

  return (
    <div
      data-redis-document={panelId}
      data-keymap-context="redisDocument"
      data-connected={opened ? "" : undefined}
      className="flex h-full min-h-0 flex-col border border-transparent text-[13px]"
      style={connectionFrameStyle(mark)}
    >
      <div className="flex flex-wrap items-center gap-2 border-b border-ui-border px-2 py-1">
        <ConnectionMark name={name} production={production} readOnly={readOnly} color={color} />
        {opened && <TlsMark tls={opened.server.tls} />}
        <label className="flex items-center gap-1">
          <span className="text-ui-muted-foreground">DB</span>
          <select
            data-testid="redis-database"
            aria-label={T.bar.database}
            value={spec.database}
            disabled={connecting}
            onChange={(e) => switchDatabase(e.target.value)}
            className="rounded-sm border border-ui-border bg-transparent px-1"
          >
            {databases.map((d) => (
              <option key={d} value={d}>
                {d}
              </option>
            ))}
          </select>
        </label>
        {opened ? (
          <button type="button" data-action="disconnect" onClick={disconnect}>
            {T.bar.disconnect}
          </button>
        ) : (
          <button
            type="button"
            data-action="connect"
            disabled={connecting || !client || !projectId}
            onClick={() => void connect()}
            className="rounded-sm border border-ui-accent px-2 disabled:opacity-50"
          >
            {connecting ? T.bar.connecting : T.bar.connect}
          </button>
        )}
        <input
          data-testid="redis-pattern"
          aria-label={T.bar.pattern}
          placeholder={T.bar.patternPlaceholder}
          value={pattern}
          spellCheck={false}
          autoComplete="off"
          onChange={(e) => setPattern(e.target.value)}
          onKeyDown={(e) => {
            if (e.key !== "Enter" || e.nativeEvent.isComposing) return;
            e.preventDefault();
            if (sessionId) startScan(sessionId, pattern);
          }}
          className="min-w-[12rem] flex-1 rounded-sm border border-ui-border bg-transparent px-1 font-mono outline-none select-text focus:border-ui-accent"
        />
        <label className="flex items-center gap-1">
          <span className="text-ui-muted-foreground">{T.bar.separator}</span>
          <input
            data-testid="redis-separator"
            aria-label={T.bar.separator}
            value={separator}
            spellCheck={false}
            autoComplete="off"
            onChange={(e) => {
              setSeparator(e.target.value);
              tree.setSeparator(e.target.value);
            }}
            className="w-10 rounded-sm border border-ui-border bg-transparent px-1 font-mono outline-none select-text focus:border-ui-accent"
          />
        </label>
        {scanState.kind === "scanning" ? (
          <button
            type="button"
            data-action="stop-scan"
            title={T.bar.stop}
            onClick={() => scan.stop()}
          >
            <Square size={14} aria-label={T.bar.stop} />
          </button>
        ) : (
          <button
            type="button"
            data-action="rescan"
            title={T.bar.rescan}
            disabled={!sessionId}
            onClick={() => sessionId && startScan(sessionId, pattern)}
            className="disabled:opacity-50"
          >
            <RotateCw size={14} aria-label={T.bar.rescan} />
          </button>
        )}
        <span
          data-testid="redis-scan-status"
          data-scan={scanState.kind}
          className="text-ui-muted-foreground"
        >
          {scanText}
        </span>
      </div>
      {!opened && (
        <div className="border-b border-ui-border px-2 py-1">
          <p data-testid="redis-not-connected" className="text-ui-muted-foreground">
            {T.bar.notConnected}
          </p>
          {connectError !== undefined &&
            (dbRefusal(connectError) ? (
              <DbRefusalMessage error={connectError} />
            ) : (
              <p role="alert" data-testid="redis-connect-error" className="text-ui-accent">
                {T.bar.connectFailed(errorText(connectError))}
              </p>
            ))}
        </div>
      )}
      <div className="flex min-h-0 flex-1">
        <div className="flex w-[320px] min-w-[200px] flex-none resize-x flex-col overflow-hidden border-r border-ui-border">
          <KeyTreeView
            tree={tree}
            loading={scanState.kind === "scanning"}
            selected={selected}
            onSelect={setSelected}
            actions={keyActions}
            readOnly={readOnly || !opened}
          />
        </div>
        <div className="flex min-w-0 flex-1 flex-col">
          {opened && client && viewing && !viewing.gone ? (
            <ValueView
              key={`${viewing.name}\n${valueRound}`}
              client={client}
              sessionId={opened.session.sessionId}
              entry={viewing}
              actions={keyActions}
              readOnly={readOnly}
            />
          ) : (
            <p className="p-3 text-ui-muted-foreground">{T.viewer.pickKey}</p>
          )}
        </div>
      </div>
      <div className="flex h-[180px] min-h-0 flex-none flex-col">
        <KvConsole session={consoleSession} />
      </div>
      {confirmElement}
      {access.element}
      {prompt && <KeyPromptDialog question={prompt.question} onAnswer={prompt.answer} />}
    </div>
  );
}
