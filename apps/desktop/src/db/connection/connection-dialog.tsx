// 연결 편집 다이얼로그(14-m3-plan.md M3-14, 10-ui.md 「다이얼로그 · 팝업 목록」 의 DB 행): 일반 · SSH 터널 · SSL · 옵션 탭.
//
// - 칸은 생성물 schema.json 의 `DbConnectionConfig` 에서 읽는다(form.ts). 상태는 react-hook-form(10-ui.md 「UI 스택」 의 폼 행).
//   드라이버를 바꾸면 그 드라이버의 칸 · 탭만 보인다. 판정은 Host 가 한다 — 저장이 거절하면 그 글을 그대로 보인다(호스트 · 터널
//   목적지의 글자 목록은 `pw_db_driver::tunnel::check_destination` 가 정본, 화면은 따로 검사하지 않는다).
// - **비밀번호 칸**: `type=password`, 자동 완성 끔, 제어하지 않는 칸(ref) — 값은 React 상태 · DOM 속성 · 폼 값에 없고, 저장 때
//   `db.secret.set` 과 [연결 테스트] 요청에만 실린다(알림 · 로그에 싣지 않는다). 다시 열면 값이 아니라 「저장됨」 표시만.
// - `sslmode`: 처음 값은 호스트로 정하고(결정 M3-D10, form.ts `defaultSslMode`) 사람이 고르기 전까지 호스트를 따라 바뀐다. 드라이버가
//   받지 않는 값(`Capabilities.sslModes` — `db.driver.list`)은 흐리게 하고 까닭을 툴팁으로 보인다. `allow` · `prefer` · `require` 를
//   고르면 「서버 인증서를 검증하지 않는다」 를 칸 옆에 적는다.
// - [연결 테스트] 는 저장 전 초안이다(`db.connection.test { draft, secret }`) — 파일을 쓰지 않는다(crates/pw-db/tests/dialog.rs).
// - 목록의 `stateProblem`(확인 기록 `db.json` 이 깨짐)이 있으면 비밀번호를 묻기 전에 그 문제와 기록 파일 경로를 먼저 보인다
//   (M3-02 재리뷰 N2 — 그동안 `secretRequired` 는 그 탓일 수 있다).

import * as Dialog from "@radix-ui/react-dialog";
import {
  type DbConnectionEntry,
  type DbConnectionList,
  type DbApprovalRequest,
  type DbDriverEntry,
  type DriverKind,
  type JsonValue,
  RpcCallError,
  type RpcClient,
  type ServerInfo,
  type SslMode,
} from "@pitwall/rpc-client";
import { type ReactNode, useEffect, useRef, useState } from "react";
import { type UseFormRegisterReturn, useForm } from "react-hook-form";

import { useKeymapPopup } from "../../keymap/keymap-provider";
import type { NotificationInput } from "../../notifications/notifications";
import { DB_TAG_TOKENS } from "../../theme/tokens";
import { ConnectionMark, TlsMark } from "../safety/connection-mark";
import { DB_ERROR_CODE, DbRefusalMessage, dbRefusal } from "../safety/messages";
import {
  type ConfigKey,
  type ConnectionForm,
  type FieldSpec,
  type TabId,
  configOf,
  defaultPort,
  defaultSslMode,
  fieldApplies,
  fieldsFromSchema,
  formOf,
  idFromName,
  loadUpsertSchema,
  newForm,
  secretTarget,
  tabsFor,
  UNVERIFIED_MODES,
} from "./form";
import { type DbConnectionStrings, useDbConnectionStrings } from "./strings";

export const SHEET =
  "sheet-top fixed left-1/2 flex max-h-[86vh] w-[560px] max-w-[94vw] -translate-x-1/2 flex-col gap-3 rounded-b-md border border-t-0 border-ui-border bg-ui-panel px-4 py-3 text-ui-foreground shadow-lg";
export const BUTTON =
  "rounded-sm border border-ui-border px-3 py-1 hover:bg-ui-hover disabled:opacity-50";
const INPUT =
  "min-w-0 flex-1 rounded-sm border border-ui-border bg-ui-background px-2 py-0.5 disabled:opacity-50";
const HINT = "text-[11px] text-ui-muted-foreground";

export interface ConnectionDialogDeps {
  client: RpcClient;
  projectId: string;
  worktree?: string;
  notify(input: NotificationInput): void;
}

/** RPC 오류의 사람 글: invalid params 는 `data`(Host 가 거절한 까닭), DB 오류(3003)는 `data.error.message`. */
export function errorText(e: unknown): string {
  if (e instanceof RpcCallError) {
    const data = e.data as JsonValue | undefined;
    if (typeof data === "string" && data !== "") return data;
    if (data && typeof data === "object" && !Array.isArray(data)) {
      const inner = data.error;
      if (inner && typeof inner === "object" && !Array.isArray(inner)) {
        if (typeof inner.message === "string") return inner.message;
      }
      if (typeof data.reason === "string" && typeof data.path === "string")
        return `${data.reason} (${data.path})`;
    }
    return e.message;
  }
  return e instanceof Error ? e.message : String(e);
}

/** 드라이버가 받지 않는 `sslmode` 의 까닭(툴팁). */
export function unsupportedReason(
  T: DbConnectionStrings["ssl"],
  driver: DriverKind,
  driverName: string,
  mode: SslMode,
): string {
  if (driver === "mysql" && mode === "verify-ca") return T.unsupportedMysqlVerifyCa;
  return T.unsupported(driverName, mode);
}

type Loaded = {
  fields: Map<ConfigKey, FieldSpec>;
  /** 등록된 드라이버(읽지 못했으면 `undefined` — 그때는 값을 흐리게 하지 않고 Host 가 판정한다). */
  drivers: DbDriverEntry[] | undefined;
  list: DbConnectionList;
  hosts: string[] | undefined;
};

type Load =
  | { kind: "loading" }
  | { kind: "failed"; message: string }
  | { kind: "ready"; loaded: Loaded };

/**
 * 연결 편집 다이얼로그. `connectionId` 가 없으면 「연결 추가」 다. 연결 설정 · 드라이버 · 스키마 · `~/.ssh/config` 호스트(신뢰한
 * 프로젝트만)를 읽은 뒤 편집기를 그린다.
 */
export function ConnectionDialog({
  deps,
  connectionId,
  onClose,
}: {
  deps: ConnectionDialogDeps;
  connectionId?: string;
  onClose(): void;
}) {
  const T = useDbConnectionStrings();
  useKeymapPopup(true);
  const [load, setLoad] = useState<Load>({ kind: "loading" });
  const latest = useRef(deps);
  latest.current = deps;

  useEffect(() => {
    let stale = false;
    const { client, projectId, worktree } = latest.current;
    const base = { projectId, worktree: worktree ?? null };
    void (async () => {
      try {
        const [schema, drivers, list] = await Promise.all([
          loadUpsertSchema(),
          client.call("db.driver.list", null).then(
            (r) => r,
            (e: unknown) => {
              console.warn("[db] driver list unavailable", e);
              return undefined;
            },
          ),
          client.call("db.connection.list", base),
        ]);
        const hosts = list.trusted
          ? await client.call("db.ssh.hosts", base).then(
              (r) => r.hosts,
              (e: unknown) => {
                console.warn("[db] ssh hosts unavailable", e);
                return [];
              },
            )
          : undefined;
        if (!stale)
          setLoad({
            kind: "ready",
            loaded: { fields: fieldsFromSchema(schema), drivers, list, hosts },
          });
      } catch (e) {
        if (!stale) setLoad({ kind: "failed", message: errorText(e) });
      }
    })();
    return () => {
      stale = true;
    };
  }, []);

  const entry =
    load.kind === "ready" && connectionId !== undefined
      ? load.loaded.list.connections.find((c) => c.id === connectionId)
      : undefined;
  const title =
    connectionId === undefined ? T.title.new : T.title.edit(entry?.config?.name ?? connectionId);

  let body: ReactNode;
  if (load.kind === "loading") body = <p className={HINT}>{T.loading}</p>;
  else if (load.kind === "failed")
    body = (
      <p role="alert" className="break-all text-ui-accent">
        {T.loadFailed(load.message)}
      </p>
    );
  else if (load.loaded.list.problem !== null)
    body = (
      <p role="alert" className="break-all text-ui-accent">
        {T.configProblem(load.loaded.list.problem)}
      </p>
    );
  else if (connectionId !== undefined && !entry)
    body = (
      <p role="alert" className="text-ui-accent">
        {T.notFound(connectionId)}
      </p>
    );
  else if (entry && !entry.config)
    body = (
      <p role="alert" className="break-all text-ui-accent">
        {T.invalid(entry.state.kind === "invalid" ? entry.state.message : "")}
      </p>
    );
  else
    body = (
      <ConnectionEditor
        deps={deps}
        loaded={load.loaded}
        entry={
          entry as
            | (DbConnectionEntry & { config: NonNullable<DbConnectionEntry["config"]> })
            | undefined
        }
        onClose={onClose}
      />
    );

  return (
    <Dialog.Root open onOpenChange={(open) => !open && onClose()}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 bg-black/20" />
        <Dialog.Content data-testid="db-connection-dialog" className={SHEET}>
          <Dialog.Title className="text-sm font-semibold">{title}</Dialog.Title>
          <Dialog.Description className="sr-only">{title}</Dialog.Description>
          {body}
          {load.kind !== "ready" && (
            <div className="flex justify-end">
              <button type="button" className={BUTTON} onClick={onClose}>
                {T.button.cancel}
              </button>
            </div>
          )}
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

type TestResult =
  | { kind: "running" }
  | { kind: "ok"; info: ServerInfo }
  | { kind: "failed"; text: string; error: unknown };

function approvalKind(error: unknown): DbApprovalRequest["kind"] | undefined {
  if (!(error instanceof RpcCallError) || error.code !== DB_ERROR_CODE.APPROVAL_REQUIRED)
    return undefined;
  const data = error.data;
  if (!data || typeof data !== "object" || Array.isArray(data)) return undefined;
  const request = data.request;
  if (!request || typeof request !== "object" || Array.isArray(request)) return undefined;
  return typeof request.kind === "string" ? (request.kind as DbApprovalRequest["kind"]) : undefined;
}

/** [연결 테스트] 실패의 글. */
export function testErrorText(T: DbConnectionStrings, e: unknown): string {
  if (e instanceof RpcCallError && e.code === DB_ERROR_CODE.SECRET_REQUIRED) return T.test.secret;
  const kind = approvalKind(e);
  if (kind) return T.test.approval(kind);
  return T.test.failed(errorText(e));
}

/** 저장한 비밀번호에 대해 지금 말할 수 있는 것(값은 읽지 않는다 — 목록의 상태로). */
function storedPasswordNote(
  T: DbConnectionStrings["password"],
  entry: DbConnectionEntry | undefined,
): string | undefined {
  if (!entry || entry.config?.password?.kind !== "keychain") return undefined;
  switch (entry.state.kind) {
    case "ready":
      return T.stored;
    case "secretRequired":
      return entry.state.reason === "missing"
        ? T.missing
        : entry.state.reason === "targetChanged"
          ? T.targetChanged
          : T.unavailable;
    case "approvalRequired":
      return T.afterApproval;
    case "untrusted":
    case "invalid":
      return undefined;
  }
}

function ConnectionEditor({
  deps,
  loaded,
  entry,
  onClose,
}: {
  deps: ConnectionDialogDeps;
  loaded: Loaded;
  entry: (DbConnectionEntry & { config: NonNullable<DbConnectionEntry["config"]> }) | undefined;
  onClose(): void;
}) {
  const T = useDbConnectionStrings();
  const editing = entry !== undefined;
  const { fields, drivers, list, hosts } = loaded;
  const trusted = list.trusted;
  // 처음 한 번 — 다시 그려도 사람이 고친 값을 덮지 않는다.
  const [initial] = useState(() =>
    entry
      ? formOf(entry)
      : newForm(
          idFromName(
            "",
            list.connections.map((c) => c.id),
          ),
        ),
  );
  const { register, watch, setValue, getValues, handleSubmit, formState } = useForm<ConnectionForm>(
    {
      defaultValues: initial,
    },
  );
  const [tab, setTab] = useState<TabId>("general");
  const [test, setTest] = useState<TestResult>();
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<unknown>();
  const [direct, setDirect] = useState(() => {
    const ref = initial.hostRef;
    return ref !== "" && !(hosts ?? []).includes(ref);
  });
  const password = useRef<HTMLInputElement>(null);
  // 사람이 sslmode 를 골랐는가. 저장한 연결이 sslmode 를 적어 두었으면 고른 것으로 본다.
  const sslChosen = useRef(editing && entry.config.sslmode != null);
  const idTouched = useRef(editing);

  const driver = watch("driver");
  const host = watch("host");
  const name = watch("name");
  const id = watch("id");
  const sslmode = watch("sslmode");
  const tunnel = watch("tunnel");
  const passwordSource = watch("passwordSource");
  const color = watch("color");
  const production = watch("production");
  const readOnly = watch("readOnly");
  const hostRef = watch("hostRef");

  // 고르기 전까지 sslmode 는 호스트를 따른다(결정 M3-D10).
  useEffect(() => {
    if (!sslChosen.current) setValue("sslmode", defaultSslMode(host));
  }, [host, setValue]);
  // 새 연결의 id 는 이름을 따른다(사람이 고치기 전까지).
  useEffect(() => {
    if (!idTouched.current)
      setValue(
        "id",
        idFromName(
          name,
          list.connections.map((c) => c.id),
        ),
      );
  }, [name, list, setValue]);

  const info = drivers?.find((d) => d.kind === driver);
  const accepted: readonly SslMode[] | undefined = info?.capabilities.sslModes;
  const driverName = T.field.driverName(driver);
  const sslValues = (fields.get("sslmode")?.values ?? []) as SslMode[];
  const unsupportedChosen =
    driver !== "sqlite" && accepted !== undefined && !accepted.includes(sslmode);
  const tabs = tabsFor(driver);
  const visibleTab = tabs.includes(tab) ? tab : "general";
  const applies = (key: ConfigKey) => fields.has(key) && fieldApplies(driver, key);
  const network = driver !== "sqlite";
  const keychain = network && passwordSource === "keychain";
  const note = storedPasswordNote(T.password, entry);
  const keepAutomaticSslMode = () => editing && entry.config.sslmode == null && !sslChosen.current;

  const base = { projectId: deps.projectId, worktree: deps.worktree ?? null };
  const typedPassword = () => (keychain ? password.current?.value || null : null);

  const runTest = async () => {
    const values = getValues();
    const draft = configOf(values, { keepAutomaticSslMode: keepAutomaticSslMode() });
    // 이름이 비어도 테스트는 돈다 — Host 는 빈 이름의 설정을 받지 않는다.
    if (draft.name === "") draft.name = values.id.trim() || "draft";
    setTest({ kind: "running" });
    try {
      const info = await deps.client.call("db.connection.test", {
        ...base,
        connectionId: editing ? entry.id : null,
        draft,
        secret: typedPassword(),
      });
      setTest({ kind: "ok", info });
    } catch (e) {
      setTest({ kind: "failed", text: testErrorText(T, e), error: e });
    }
  };

  const save = handleSubmit(async (values) => {
    if (unsupportedChosen) return;
    setSaving(true);
    setSaveError(undefined);
    const connectionId = values.id.trim();
    const config = configOf(values, { keepAutomaticSslMode: keepAutomaticSslMode() });
    let saved: DbConnectionEntry;
    try {
      saved = await deps.client.call("db.connection.upsert", {
        ...base,
        connectionId,
        config,
        location: values.location,
      });
    } catch (e) {
      setSaving(false);
      setSaveError(e);
      return;
    }
    const typed = typedPassword();
    try {
      if (typed && saved.config) {
        await deps.client.call("db.secret.set", {
          ...base,
          connectionId,
          key: "password",
          target: secretTarget(saved.config),
          value: typed,
        });
      }
    } catch (e) {
      setSaving(false);
      setSaveError({ secret: e });
      return;
    }
    if (password.current) password.current.value = "";
    if (
      editing &&
      entry.config.password?.kind === "keychain" &&
      config.password?.kind !== "keychain"
    ) {
      // 키체인을 더 쓰지 않으면 남은 항목을 지운다(신뢰와 상관없이 지울 수 있다).
      await deps.client
        .call("db.secret.clear", { projectId: deps.projectId, connectionId, key: "password" })
        .catch((e: unknown) => console.warn("[db] keychain item not cleared", connectionId, e));
    }
    deps.notify({ level: "info", title: T.saved(config.name) });
    onClose();
  });

  const required = (field: string) => ({ required: T.field.required(field) });
  // 드라이버에 따라 숨는 칸은 그 드라이버일 때만 채워야 한다.
  const requiredIf = (field: string, when: (all: ConnectionForm) => boolean) => ({
    validate: (v: string, all: ConnectionForm) =>
      !when(all) || v.trim() !== "" || T.field.required(field),
  });
  const error = (key: keyof ConnectionForm) => {
    const message = formState.errors[key]?.message;
    return message ? (
      <p role="alert" data-error={key} className="text-[11px] text-ui-accent">
        {message}
      </p>
    ) : null;
  };

  const tabButton = (t: TabId) => (
    <button
      key={t}
      type="button"
      role="tab"
      data-tab={t}
      aria-selected={visibleTab === t}
      className={`rounded-t-sm border-b-2 px-3 py-1 ${visibleTab === t ? "border-ui-accent" : "border-transparent text-ui-muted-foreground"}`}
      onClick={() => setTab(t)}
    >
      {T.tab[t]}
    </button>
  );

  const portSpec = fields.get("port");
  const port = defaultPort(driver);

  return (
    <form
      data-testid="db-connection-form"
      className="flex min-h-0 flex-col gap-3"
      autoComplete="off"
      onSubmit={(e) => void save(e)}
    >
      {list.stateProblem !== null && (
        <p role="alert" data-testid="db-conn-state-problem" className="break-all text-ui-accent">
          {T.password.stateProblem(list.stateProblem)}
        </p>
      )}
      <div role="tablist" className="flex gap-1 border-b border-ui-border">
        {tabs.map(tabButton)}
      </div>

      {/* 탭은 모두 그려 두고 숨긴다 — 비밀번호 칸(제어하지 않는 칸)의 값이 탭을 바꿔도 남게. */}
      <div
        role="tabpanel"
        data-panel="general"
        hidden={visibleTab !== "general"}
        className="flex flex-col gap-2"
      >
        <Row label={T.field.name}>
          <input
            className={INPUT}
            {...register("name", required(T.field.name))}
            data-field="name"
          />
        </Row>
        {error("name")}
        <Row label={T.field.id} hint={T.field.idHint}>
          <input
            className={INPUT}
            readOnly={editing}
            {...withChange(register("id", required(T.field.id)), () => (idTouched.current = true))}
            data-field="id"
          />
        </Row>
        {error("id")}
        {fields.has("driver") && (
          <Row label={T.field.driver}>
            <select className={INPUT} {...register("driver")} data-field="driver">
              {(fields.get("driver")?.values ?? []).map((d) => (
                <option
                  key={d}
                  value={d}
                  disabled={drivers !== undefined && !drivers.some((i) => i.kind === d)}
                >
                  {T.field.driverName(d)}
                </option>
              ))}
            </select>
          </Row>
        )}
        {applies("host") && (
          <Row label={T.field.host} hint={tunnel ? T.field.hostTunnel : undefined}>
            <input
              className={INPUT}
              {...register(
                "host",
                requiredIf(T.field.host, (all) => all.driver !== "sqlite"),
              )}
              data-field="host"
            />
          </Row>
        )}
        {applies("host") && error("host")}
        {applies("port") && (
          <Row label={T.field.port}>
            <input
              className={INPUT}
              inputMode="numeric"
              placeholder={port === null ? "" : T.field.portDefault(String(port))}
              {...(portSpec?.minimum !== undefined && { min: portSpec.minimum })}
              {...(portSpec?.maximum !== undefined && { max: portSpec.maximum })}
              {...register("port")}
              data-field="port"
            />
          </Row>
        )}
        {applies("database") && (
          <Row label={driver === "redis" ? T.field.databaseRedis : T.field.database}>
            <input className={INPUT} {...register("database")} data-field="database" />
          </Row>
        )}
        {applies("user") && (
          <Row label={T.field.user}>
            <input className={INPUT} {...register("user")} data-field="user" />
          </Row>
        )}
        {applies("path") && (
          <Row label={T.field.path} hint={T.field.pathHint}>
            <input
              className={INPUT}
              {...register(
                "path",
                requiredIf(T.field.path, (all) => all.driver === "sqlite"),
              )}
              data-field="path"
            />
          </Row>
        )}
        {applies("path") && error("path")}
        {applies("password") && (
          <fieldset className="flex flex-col gap-1" data-testid="db-conn-password-group">
            <Row label={T.password.label}>
              <select className={INPUT} {...register("passwordSource")} data-field="passwordSource">
                <option value="none">{T.password.source.none}</option>
                <option value="keychain">{T.password.source.keychain}</option>
                <option value="env">{T.password.source.env}</option>
              </select>
            </Row>
            {/* 칸은 늘 그려 두고(값이 남게) 키체인일 때만 보인다. */}
            <div hidden={!keychain} className="flex flex-col gap-1">
              <Row label="">
                <input
                  ref={password}
                  type="password"
                  name="db-connection-password"
                  autoComplete="off"
                  spellCheck={false}
                  disabled={!trusted}
                  aria-label={T.password.label}
                  data-testid="db-conn-password"
                  className={INPUT}
                />
              </Row>
              <p className={HINT} data-testid="db-conn-password-note">
                {!trusted ? T.password.untrusted : (note ?? T.password.hint)}
              </p>
            </div>
            {network && passwordSource === "env" && (
              <>
                <Row label={T.password.envVariable}>
                  <input
                    className={INPUT}
                    spellCheck={false}
                    {...register(
                      "envVariable",
                      requiredIf(
                        T.password.envVariable,
                        (all) => all.driver !== "sqlite" && all.passwordSource === "env",
                      ),
                    )}
                    data-field="envVariable"
                  />
                </Row>
                {error("envVariable")}
                <p className={HINT}>{T.password.envHint}</p>
              </>
            )}
          </fieldset>
        )}
      </div>

      <div
        role="tabpanel"
        data-panel="ssh"
        hidden={visibleTab !== "ssh"}
        className="flex flex-col gap-2"
      >
        <label className="flex items-center gap-2">
          <input type="checkbox" {...register("tunnel")} data-field="tunnel" />
          {T.ssh.use}
        </label>
        {tunnel && (
          <>
            <Row label={T.ssh.host}>
              <select
                className={INPUT}
                value={direct ? "" : hostRef}
                disabled={hosts === undefined || hosts.length === 0}
                data-field="hostPick"
                onChange={(e) => {
                  if (e.target.value === "") setDirect(true);
                  else {
                    setDirect(false);
                    setValue("hostRef", e.target.value, { shouldDirty: true });
                  }
                }}
              >
                <option value="">{T.ssh.direct}</option>
                {(hosts ?? []).map((h) => (
                  <option key={h} value={h}>
                    {h}
                  </option>
                ))}
              </select>
            </Row>
            <Row label="">
              <input
                className={INPUT}
                spellCheck={false}
                placeholder={T.ssh.directPlaceholder}
                readOnly={!direct && (hosts ?? []).length > 0}
                {...register(
                  "hostRef",
                  requiredIf(T.ssh.host, (all) => all.driver !== "sqlite" && all.tunnel),
                )}
                data-field="hostRef"
              />
            </Row>
            {error("hostRef")}
            <p className={HINT}>
              {hosts === undefined
                ? T.ssh.untrusted
                : hosts.length === 0
                  ? T.ssh.noHosts
                  : T.ssh.pick}
            </p>
          </>
        )}
        <p className={HINT}>{T.ssh.hint}</p>
      </div>

      <div
        role="tabpanel"
        data-panel="ssl"
        hidden={visibleTab !== "ssl"}
        className="flex flex-col gap-2"
      >
        <Row label={T.ssl.mode} hint={keepAutomaticSslMode() ? T.ssl.auto : undefined}>
          <select
            className={INPUT}
            {...withChange(register("sslmode"), () => (sslChosen.current = true))}
            data-field="sslmode"
          >
            {sslValues.map((mode) => {
              const refused = accepted !== undefined && !accepted.includes(mode);
              return (
                <option
                  key={mode}
                  value={mode}
                  disabled={refused}
                  data-unsupported={refused ? "true" : undefined}
                  title={refused ? unsupportedReason(T.ssl, driver, driverName, mode) : undefined}
                >
                  {mode}
                </option>
              );
            })}
          </select>
        </Row>
        {UNVERIFIED_MODES.has(sslmode) && (
          <p
            data-testid="db-conn-ssl-warning"
            data-warning="unverified"
            className="text-db-insecure"
          >
            {T.ssl.unverified}
          </p>
        )}
        {sslmode === "disable" && defaultSslMode(host) !== "disable" && (
          <p
            data-testid="db-conn-ssl-warning"
            data-warning="plaintext"
            className="text-db-insecure"
          >
            {T.ssl.plaintext}
          </p>
        )}
        {accepted !== undefined && sslValues.some((m) => !accepted.includes(m)) && (
          <ul data-testid="db-conn-ssl-unsupported-list" className={HINT}>
            {sslValues
              .filter((m) => !accepted.includes(m))
              .map((m) => (
                <li key={m} data-mode={m}>
                  {m}: {unsupportedReason(T.ssl, driver, driverName, m)}
                </li>
              ))}
          </ul>
        )}
        {unsupportedChosen && (
          <p role="alert" data-testid="db-conn-ssl-unsupported" className="text-ui-accent">
            {T.ssl.unsupportedChosen} — {unsupportedReason(T.ssl, driver, driverName, sslmode)}
          </p>
        )}
        {(["sslrootcert", "sslcert", "sslkey"] as const).map((key) => (
          <Row
            key={key}
            label={T.ssl[key === "sslrootcert" ? "rootcert" : key === "sslcert" ? "cert" : "key"]}
          >
            <input className={INPUT} spellCheck={false} {...register(key)} data-field={key} />
          </Row>
        ))}
        <p className={HINT}>{T.ssl.pathHint}</p>
      </div>

      <div
        role="tabpanel"
        data-panel="options"
        hidden={visibleTab !== "options"}
        className="flex flex-col gap-2"
      >
        <label className="flex items-center gap-2" title={T.options.readOnlyHint}>
          <input type="checkbox" {...register("readOnly")} data-field="readOnly" />
          {T.options.readOnly}
        </label>
        <label className="flex items-center gap-2" title={T.options.productionHint}>
          <input type="checkbox" {...register("production")} data-field="production" />
          {T.options.production}
        </label>
        <p className={HINT}>{T.options.productionHint}</p>
        <Row label={T.options.color}>
          <select className={INPUT} {...register("color")} data-field="color">
            <option value="">{T.options.colorNone}</option>
            {DB_TAG_TOKENS.map((tag) => (
              <option key={tag} value={tag}>
                {T.options.colorTag(tag)}
              </option>
            ))}
            {color !== "" && !(DB_TAG_TOKENS as readonly string[]).includes(color) && (
              <option value={color}>{color}</option>
            )}
          </select>
          <ConnectionMark production={production} readOnly={readOnly} color={color || null} />
        </Row>
        <Row label={T.options.connectTimeout} hint={T.options.connectTimeoutHint}>
          <input
            className={INPUT}
            inputMode="numeric"
            {...register("connectTimeout")}
            data-field="connectTimeout"
          />
        </Row>
      </div>

      <fieldset
        className="flex flex-col gap-1 border-t border-ui-border pt-2"
        data-testid="db-conn-location"
      >
        <span>{T.location.label}</span>
        <label className="flex items-center gap-2">
          <input type="radio" value="shared" {...register("location")} />
          {T.location.shared(id || "<id>")}
        </label>
        <label className="flex items-center gap-2">
          <input type="radio" value="local" {...register("location")} />
          {T.location.local(id || "<id>")}
        </label>
        <p className={HINT}>{T.location.localHint}</p>
      </fieldset>

      {test && (
        <div
          data-testid="db-conn-test-result"
          data-result={test.kind}
          className="flex flex-col gap-1"
        >
          {test.kind === "running" && <p className={HINT}>{T.button.testing}</p>}
          {test.kind === "ok" && (
            <>
              <p>{test.info.version ? T.test.ok(test.info.version) : T.test.okNoVersion}</p>
              <p className={HINT} data-tls={test.info.tls}>
                {T.test.tls(test.info.tls)} <TlsMark tls={test.info.tls} />
              </p>
            </>
          )}
          {test.kind === "failed" &&
            (dbRefusal(test.error) ? (
              <DbRefusalMessage error={test.error} />
            ) : (
              <p role="alert" className="break-all text-ui-accent">
                {test.text}
              </p>
            ))}
        </div>
      )}
      {saveError !== undefined && (
        <p role="alert" data-testid="db-conn-save-error" className="break-all text-ui-accent">
          {typeof saveError === "object" && saveError !== null && "secret" in saveError
            ? T.secretFailed(errorText((saveError as { secret: unknown }).secret))
            : T.saveFailed(errorText(saveError))}
        </p>
      )}

      <div className="flex items-center justify-end gap-2">
        <span className={`mr-auto ${HINT}`}>{T.test.draft}</span>
        <button
          type="button"
          className={BUTTON}
          disabled={!trusted || test?.kind === "running" || unsupportedChosen}
          data-testid="db-conn-test"
          onClick={() => void runTest()}
        >
          {test?.kind === "running" ? T.button.testing : T.button.test}
        </button>
        <button type="button" className={BUTTON} onClick={onClose}>
          {T.button.cancel}
        </button>
        <button
          type="submit"
          className={`${BUTTON} border-ui-accent`}
          disabled={saving || unsupportedChosen}
          data-testid="db-conn-save"
        >
          {saving ? T.button.saving : T.button.save}
        </button>
      </div>
    </form>
  );
}

/** react-hook-form 의 `onChange` 에 한 가지를 더 끼운다. */
function withChange<N extends string>(
  registered: UseFormRegisterReturn<N>,
  extra: () => void,
): UseFormRegisterReturn<N> {
  return {
    ...registered,
    onChange: (event) => {
      extra();
      return registered.onChange(event);
    },
  };
}

function Row({
  label,
  hint,
  children,
}: {
  label: string;
  hint?: string | undefined;
  children: ReactNode;
}) {
  return (
    <div className="flex flex-col gap-0.5">
      <label className="flex items-center gap-2">
        <span className="w-28 flex-none text-ui-muted-foreground">{label}</span>
        {children}
      </label>
      {hint && <p className={`pl-30 ${HINT}`}>{hint}</p>}
    </div>
  );
}
