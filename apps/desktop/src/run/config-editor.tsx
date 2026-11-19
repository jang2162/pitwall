// 실행 구성 편집(17-m6-plan M6-07 ③ — 10-ui.md §2.6 「실행 구성 편집(폼 + TOML 보기)」).
//
// - 폼은 스키마에서(config-form.ts — `RunConfig` 의 `oneOf` 갈래), 상태는 react-hook-form(10-ui.md 「UI 스택」 의 폼 행).
// - 「TOML 보기」 는 읽기 전용이고 **저장하면 생길 글 그대로**다(`run.config.preview` — 저장과 같은 `pw_run::config::plan`, 주석 ·
//   모르는 키를 지킨 결과).
// - 저장은 `run.config.save` 하나(공유 `.pitwall/run/<id>.toml` · 개인 `<id>.local.toml` 고르기). 그 프로젝트 창이 저장한 구성은
//   Host 가 「사람이 본 구성」 으로 기록한다(M6-07 ④ — 처음 띄울 때 묻지 않는다). 지우기는 `run.config.delete`(git 이 추적하는 개인
//   파일이면 Host 가 거절한다 — ⑤).
// - 감지된 태스크의 「구성으로 저장」 은 그 태스크의 구성 값으로 새 구성 편집을 연다(`openRunConfigEditor`).

import type { RunConfig } from "@pitwall/rpc-client";
import * as Dialog from "@radix-ui/react-dialog";
import { X } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { useFieldArray, useForm } from "react-hook-form";
import { useStore } from "zustand";
import { createStore } from "zustand/vanilla";

import { useKeymapPopup } from "../keymap/keymap-provider";
import {
  configOf,
  loadSaveSchema,
  missingRequired,
  type RunConfigForm,
  type RunFieldSpec,
  type RunFormSchema,
  runFormSchema,
} from "./config-form";
import { errorText, type RunConfigs, windowRunConfigs } from "./run-configs";
import { useRunStrings } from "./strings";

const BUTTON = "rounded-sm border border-ui-border px-3 py-1 hover:bg-ui-hover disabled:opacity-50";
const INPUT =
  "min-w-0 flex-1 rounded-sm border border-ui-border bg-ui-background px-2 py-0.5 disabled:opacity-50";
const HINT = "text-[11px] text-ui-muted-foreground";

/** 편집기를 여는 값. */
export interface RunEditorOpen {
  /** 새 구성이면 참(id 를 고칠 수 있다). */
  creating: boolean;
  form: RunConfigForm;
  /** 편집을 시작한 구성 — 폼 밖의 칸(프로필 · `[debug]`)을 그대로 보낸다. */
  kept?: RunConfig;
}

const editorStore = createStore<{ open: RunEditorOpen | undefined }>()(() => ({
  open: undefined,
}));

/** 구성 편집을 연다(실행 선택기 · 액션 · 감지된 태스크의 「구성으로 저장」). */
export function openRunConfigEditor(open: RunEditorOpen): void {
  editorStore.setState({ open });
}

/** 시험: 떠 있는 편집기를 닫는다. */
export function closeRunConfigEditor(): void {
  editorStore.setState({ open: undefined });
}

function Row({
  label,
  hint,
  children,
}: {
  label: string;
  hint?: string;
  children: React.ReactNode;
}) {
  return (
    <label className="flex flex-col gap-0.5">
      <span className="flex items-center gap-2">
        <span className="w-28 flex-none">{label}</span>
        {children}
      </span>
      {hint && <span className={`${HINT} pl-30`}>{hint}</span>}
    </label>
  );
}

/** 이 창의 구성 편집기. 열린 것이 없으면 그리지 않는다. */
export function RunConfigEditorHost({ configs = windowRunConfigs() }: { configs?: RunConfigs }) {
  const open = useStore(editorStore, (s) => s.open);
  const [schema, setSchema] = useState<RunFormSchema>();
  const [schemaError, setSchemaError] = useState<string>();
  useEffect(() => {
    if (!open || schema) return;
    let live = true;
    loadSaveSchema()
      .then((params) => live && setSchema(runFormSchema(params)))
      .catch((e: unknown) => live && setSchemaError(errorText(e)));
    return () => {
      live = false;
    };
  }, [open, schema]);
  if (!open) return null;
  // 스키마를 읽기 전에는 그리지 않는다 — 고르기 칸의 처음 값이 그 칸의 선택지가 생기기 전에 걸리면 사라진다.
  if (!schema)
    return schemaError ? (
      <p role="alert" className="text-ui-accent">
        {schemaError}
      </p>
    ) : null;
  return (
    <RunConfigEditor
      key={`${open.form.id}:${open.creating}`}
      open={open}
      schema={schema}
      schemaError={schemaError}
      configs={configs}
      onClose={() => editorStore.setState({ open: undefined })}
    />
  );
}

export function RunConfigEditor({
  open,
  schema,
  schemaError,
  configs,
  onClose,
}: {
  open: RunEditorOpen;
  schema: RunFormSchema;
  schemaError?: string;
  configs: RunConfigs;
  onClose(): void;
}) {
  const T = useRunStrings().editor;
  useKeymapPopup(true);
  const { register, control, watch, getValues, handleSubmit } = useForm<RunConfigForm>({
    defaultValues: open.form,
  });
  const env = useFieldArray({ control, name: "env" });
  const type = watch("type");
  const [view, setView] = useState<"form" | "toml">("form");
  const [toml, setToml] = useState<{ path: string; text: string } | { error: string }>();
  const [failure, setFailure] = useState<string>();
  const [missing, setMissing] = useState<string[]>([]);
  const [busy, setBusy] = useState(false);
  const kind = useMemo(() => schema?.kinds.find((k) => k.type === type), [schema, type]);
  const place = configs.place();

  const params = () => {
    const form = getValues();
    return {
      projectId: place!.projectId,
      worktree: place!.worktree,
      id: form.id.trim(),
      config: configOf(form, schema!, open.kept),
      location: form.location,
    };
  };

  const showToml = async () => {
    setView("toml");
    if (!place || !schema) return;
    setToml(undefined);
    try {
      const preview = await place.client.call("run.config.preview", params());
      setToml({ path: preview.path, text: preview.text });
    } catch (e) {
      setToml({ error: errorText(e) });
    }
  };

  const save = handleSubmit(async (form) => {
    if (!place || !schema) return;
    const empty = missingRequired(form, schema);
    setMissing(empty);
    if (empty.length > 0) {
      setView("form");
      return;
    }
    setBusy(true);
    setFailure(undefined);
    try {
      const p = params();
      await place.client.call("run.config.save", p);
      configs.select({ id: p.id, profile: null });
      await configs.reload();
      onClose();
    } catch (e) {
      setFailure(T.saveFailed(errorText(e)));
    } finally {
      setBusy(false);
    }
  });

  const remove = async () => {
    if (!place) return;
    setBusy(true);
    setFailure(undefined);
    try {
      await place.client.call("run.config.delete", {
        projectId: place.projectId,
        worktree: place.worktree,
        id: open.form.id,
      });
      await configs.reload();
      onClose();
    } catch (e) {
      setFailure(T.deleteFailed(errorText(e)));
    } finally {
      setBusy(false);
    }
  };

  const label = (key: string) =>
    key === "id" ? T.id : key === "name" ? T.name : key === "cwd" ? T.cwd : key;
  const required = (key: string) =>
    missing.includes(key) && (
      <p role="alert" className="text-[12px] text-ui-accent" data-missing={key}>
        {T.required(label(key))}
      </p>
    );

  const kindField = (field: RunFieldSpec) => (
    <div key={`${type}:${field.key}`}>
      <Row
        label={`${field.key}${field.required ? " *" : ""}`}
        hint={field.kind === "list" ? T.listHint : undefined}
      >
        {field.kind === "enum" ? (
          <select
            className={INPUT}
            data-field={field.key}
            {...register(`kind.${field.key}`)}
            defaultValue={field.values[0]}
          >
            {field.nullable && <option value="" />}
            {field.values.map((v) => (
              <option key={v} value={v}>
                {v}
              </option>
            ))}
          </select>
        ) : field.kind === "list" ? (
          <textarea
            rows={2}
            className={`${INPUT} font-mono`}
            data-field={field.key}
            aria-required={field.required}
            {...register(`kind.${field.key}`)}
          />
        ) : (
          <input
            className={`${INPUT} font-mono`}
            data-field={field.key}
            aria-required={field.required}
            {...register(`kind.${field.key}`)}
          />
        )}
      </Row>
      {required(field.key)}
    </div>
  );

  const kept = open.kept && (open.kept.profiles || open.kept.debug);

  return (
    <Dialog.Root open onOpenChange={(o) => !o && onClose()}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 bg-black/20" />
        <Dialog.Content
          data-testid="run-config-editor"
          aria-describedby={undefined}
          className="sheet-top fixed left-1/2 flex max-h-[85vh] w-[600px] max-w-[94vw] -translate-x-1/2 flex-col gap-3 overflow-y-auto rounded-b-md border border-t-0 border-ui-border bg-ui-panel px-4 py-3 text-ui-foreground shadow-lg"
        >
          <Dialog.Title className="text-sm font-semibold">
            {open.creating ? T.titleNew : T.titleEdit(open.form.name || open.form.id)}
          </Dialog.Title>
          <div role="tablist" className="flex gap-1 border-b border-ui-border">
            {(["form", "toml"] as const).map((v) => (
              <button
                key={v}
                type="button"
                role="tab"
                data-tab={v}
                aria-selected={view === v}
                className={`rounded-t-sm border-b-2 px-3 py-1 ${view === v ? "border-ui-accent" : "border-transparent text-ui-muted-foreground"}`}
                onClick={() => (v === "toml" ? void showToml() : setView("form"))}
              >
                {v === "form" ? T.form : T.toml}
              </button>
            ))}
          </div>
          {schemaError && (
            <p role="alert" className="text-ui-accent">
              {schemaError}
            </p>
          )}
          <form
            className="flex flex-col gap-2"
            autoComplete="off"
            hidden={view !== "form"}
            data-testid="run-config-form"
            onSubmit={(e) => void save(e)}
          >
            <Row label={`${T.name} *`}>
              <input className={INPUT} data-field="name" autoFocus {...register("name")} />
            </Row>
            {required("name")}
            <Row label={`${T.id} *`} hint={T.idHint}>
              <input
                className={`${INPUT} font-mono`}
                data-field="id"
                readOnly={!open.creating}
                {...register("id")}
              />
            </Row>
            {required("id")}
            <Row label={T.type}>
              <select className={INPUT} data-field="type" {...register("type")}>
                {(schema?.kinds ?? []).map((k) => (
                  <option key={k.type} value={k.type} title={k.description}>
                    {k.type}
                  </option>
                ))}
              </select>
            </Row>
            {kind?.description && <p className={HINT}>{kind.description}</p>}
            {kind?.fields.map(kindField)}
            <Row label={T.cwd} hint={T.cwdHint}>
              <input className={`${INPUT} font-mono`} data-field="cwd" {...register("cwd")} />
            </Row>
            <fieldset className="flex flex-col gap-1">
              <legend className="mb-1">{T.env}</legend>
              {env.fields.map((field, i) => (
                <div key={field.id} className="flex items-center gap-1" data-env-row={i}>
                  <input
                    className={`${INPUT} font-mono`}
                    aria-label={T.envName}
                    placeholder={T.envName}
                    {...register(`env.${i}.name`)}
                  />
                  <input
                    className={`${INPUT} font-mono`}
                    aria-label={T.envValue}
                    placeholder={T.envValue}
                    {...register(`env.${i}.value`)}
                  />
                  <button
                    type="button"
                    className="rounded-sm p-1 hover:bg-ui-hover"
                    aria-label={T.envRemove(getValues(`env.${i}.name`))}
                    onClick={() => env.remove(i)}
                  >
                    <X size={12} aria-hidden />
                  </button>
                </div>
              ))}
              <button
                type="button"
                className={`${BUTTON} self-start`}
                onClick={() => env.append({ name: "", value: "" })}
              >
                {T.envAdd}
              </button>
            </fieldset>
            <Row label={T.envFile} hint={T.envFileHint}>
              <textarea
                rows={2}
                className={`${INPUT} font-mono`}
                data-field="envFile"
                {...register("envFile")}
              />
            </Row>
            <fieldset className="flex flex-col gap-1">
              <legend className="mb-1">{T.location}</legend>
              <label className="flex items-center gap-2">
                <input type="radio" value="shared" {...register("location")} />
                {T.locationShared}
              </label>
              <label className="flex items-center gap-2">
                <input type="radio" value="local" {...register("location")} />
                {T.locationLocal}
              </label>
            </fieldset>
            {kept && <p className={HINT}>{T.kept}</p>}
          </form>
          {view === "toml" && (
            <div className="flex flex-col gap-1" data-testid="run-config-toml">
              <p className={HINT}>{T.tomlHint}</p>
              {toml && "path" in toml && (
                <p className={`${HINT} font-mono`}>{T.tomlPath(toml.path)}</p>
              )}
              {toml && "error" in toml ? (
                <p role="alert" className="text-ui-accent">
                  {T.tomlFailed(toml.error)}
                </p>
              ) : (
                <pre
                  aria-readonly
                  className="max-h-[50vh] overflow-auto rounded-sm border border-ui-border bg-ui-background px-2 py-1 font-mono text-[12px] whitespace-pre select-text"
                >
                  {toml && "text" in toml ? toml.text : ""}
                </pre>
              )}
            </div>
          )}
          {failure && (
            <p role="alert" data-testid="run-config-failure" className="break-all text-ui-accent">
              {failure}
            </p>
          )}
          <div className="flex items-center gap-2">
            {!open.creating && (
              <button
                type="button"
                className={BUTTON}
                disabled={busy}
                onClick={() => void remove()}
              >
                {T.remove}
              </button>
            )}
            <span className="flex-1" />
            <button type="button" className={BUTTON} onClick={onClose}>
              {T.cancel}
            </button>
            <button
              type="button"
              className={`${BUTTON} border-ui-accent`}
              disabled={busy || !schema || !place}
              onClick={() => void save()}
            >
              {T.save}
            </button>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
