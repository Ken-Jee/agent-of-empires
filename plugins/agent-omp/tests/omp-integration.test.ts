import { expect, test } from "bun:test";
import initialize, { ensurePrivateDirectories, integrationDirectory, isApprovalSelection, nativeApprovalKey, observeApproval, observeApprovalSelect, observeAskDialog, supportedRuntime, validateReply } from "../omp-extension/index.ts";
import { chmodSync, mkdtempSync, readFileSync, rmSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";

const questions = [{ id: "q1", question: "Choose", options: [{ label: "one" }, { label: "two" }] }];
const answer = { kind: "submit" as const, results: [{ id: "q1", question: "Choose", options: ["one", "two"], multi: false, selectedOptions: ["two"], timedOut: false }] };
const snapshot = {
  schema_version: 1 as const,
  instance_id: "instance-a",
  provider_version: "18.11.1",
  provider_session_id: "session-a",
  writer_pid: 123,
  revision: 7,
  status: "waiting" as const,
  reply_keys: ["F12"],
  prompt: { kind: "question" as const, id: "prompt-a", title: "Choose", questions },
};
const validReply = {
  schema_version: 1,
  instance_id: "instance-a",
  provider_session_id: "session-a",
  writer_pid: 123,
  revision: 7,
  prompt_id: "prompt-a",
  response_id: "7d442dd2-7b10-4cc4-9207-123456789abc",
  response: { kind: "question", answers: [{ id: "q1", selected_options: ["two"] }] },
};

test("external question answer closes only the child dialog and returns a native submit result", async () => {
  let accept!: (value: typeof answer) => boolean;
  let nativeSettled = false;
  let closed = false;
  const parent = new AbortController();
  const dialog = observeAskDialog((_questions, options) => {
    const { promise, reject } = Promise.withResolvers<typeof answer | undefined>();
    options?.signal?.addEventListener("abort", () => {
      nativeSettled = true;
      reject(new Error("AbortError"));
    }, { once: true });
    return promise;
  }, {
    open(_id, _questions, submit) { accept = submit as (value: typeof answer) => boolean; },
    close() { closed = true; },
  });
  const result = dialog(questions, { signal: parent.signal });
  expect(accept(answer)).toBe(true);
  expect(await result).toEqual(answer);
  expect(nativeSettled).toBe(true);
  expect(parent.signal.aborted).toBe(false);
  expect(closed).toBe(true);
  expect(accept(answer)).toBe(false);
});

test("native question completion and cancellation win over a later sidebar answer", async () => {
  for (const nativeResult of [answer, undefined]) {
    let accept!: (value: typeof answer) => boolean;
    const dialog = observeAskDialog(async () => nativeResult, {
      open(_id, _questions, submit) { accept = submit as (value: typeof answer) => boolean; },
      close() {},
    });
    await expect(dialog(questions)).resolves.toEqual(nativeResult);
    expect(accept(answer)).toBe(false);
  }
});
test("outer tool cancellation propagates into the native dialog", async () => {
  let childSignal: AbortSignal | undefined;
  const parent = new AbortController();
  const dialog = observeAskDialog((_questions, options) => {
    childSignal = options?.signal;
    const { promise, resolve } = Promise.withResolvers<typeof answer | undefined>();
    childSignal?.addEventListener("abort", () => resolve(undefined), { once: true });
    return promise;
  }, { open() {}, close() {} });
  const result = dialog(questions, { signal: parent.signal });
  parent.abort();
  await expect(result).resolves.toBeUndefined();
  expect(parent.signal.aborted).toBe(true);
  expect(childSignal?.aborted).toBe(true);
});

test("rejects stale, wrong-session, wrong-prompt, and invalid question envelopes", () => {
  expect(validateReply(validReply, snapshot)).toBe(true);
  const variants = [
    { ...validReply, instance_id: "other" },
    { ...validReply, provider_session_id: "other" },
    { ...validReply, writer_pid: 456 },
    { ...validReply, revision: 6 },
    { ...validReply, prompt_id: "other" },
    { ...validReply, response: { kind: "question", answers: [{ id: "other", selected_options: ["two"] }] } },
    { ...validReply, response: { kind: "question", answers: [{ id: "q1", selected_options: ["not offered"] }] } },
    { ...validReply, response: { kind: "question", answers: [{ id: "q1", selected_options: [] }] } },
    { ...validReply, response: { kind: "question", answers: [{ id: "q1", selected_options: ["two", "two"] }] } },
    { ...validReply, response: { kind: "question", answers: [{ id: "q1", selected_options: ["two"], custom_input: "other" }] } },
    { ...validReply, response: { kind: "question", answers: [{ id: "q1", selected_options: [], custom_input: " " }] } },
  ];
  for (const value of variants) expect(validateReply(value, snapshot)).toBe(false);
});
test("allow_always replies require native prompt support", () => {
  const approvalSnapshot = {
    ...snapshot,
    prompt: { kind: "approval" as const, id: "approval-a", title: "Approve", allow_always: false },
  };
  const allowAlwaysReply = {
    ...validReply,
    prompt_id: "approval-a",
    response: { kind: "approval", choice: "allow_always" },
  };
  expect(validateReply(allowAlwaysReply, approvalSnapshot)).toBe(false);
  expect(validateReply(allowAlwaysReply, {
    ...approvalSnapshot,
    prompt: { ...approvalSnapshot.prompt, allow_always: true },
  })).toBe(true);
});

test("approval observer preserves the exact native result and key mapping is native", async () => {
  for (const nativeResult of [false, true, "once", "session", "deny"] as const) {
    const events: string[] = [];
    const result = await observeApproval(async () => nativeResult, () => events.push("open"), () => events.push("close"));
    expect(result).toBe(nativeResult);
    expect(events).toEqual(["open", "close"]);
  }
  expect(nativeApprovalKey("allow", ["ctrl+enter", "ctrl+q"])).toBe("C-Enter");
  expect(nativeApprovalKey("allow", ["ctrl+q"])).toBe("C-q");
  expect(nativeApprovalKey("deny", ["ctrl+c", "escape"])).toBe("Escape");
  expect(nativeApprovalKey("allow_always", [])).toBeUndefined();
});
test("provider creates owner-only directories and uses the shared /tmp hook path", async () => {
  const uid = process.getuid();
  const scratch = mkdtempSync(path.join(tmpdir(), "aoe-omp-integration-"));
  const root = path.join(scratch, `aoe-hooks-${uid}`);
  const dir = path.join(root, "instance-a");
  try {
    expect(await ensurePrivateDirectories(root, dir, uid)).toBe(true);
    expect(statSync(root).mode & 0o777).toBe(0o700);
    expect(statSync(dir).mode & 0o777).toBe(0o700);
    expect(integrationDirectory(uid, "instance-a")).toBe(`/tmp/aoe-hooks-${uid}/instance-a`);
    chmodSync(root, 0o1700);
    expect(await ensurePrivateDirectories(root, dir, uid)).toBe(false);
  } finally {
    rmSync(scratch, { recursive: true, force: true });
  }
});

test("runtime version boundaries and independent plugin metadata stay aligned", () => {
  const manifest = Bun.TOML.parse(readFileSync(new URL("../aoe-plugin.toml", import.meta.url), "utf8"));
  const metadata = JSON.parse(readFileSync(new URL("../omp-extension/package.json", import.meta.url), "utf8"));
  const supported = ["18.10.3", "18.10.4", "18.11.1", "18.11.3", "18.11.3+source"];
  const runtimeRange = manifest.agent_integrations[0].provider_versions
    .map((range: string) => range.replaceAll(",", "")).join(" || ");
  expect(metadata.peerDependencies["@oh-my-pi/pi-coding-agent"]).toBe(runtimeRange);
  expect(metadata.peerDependencies["@oh-my-pi/pi-tui"]).toBe(runtimeRange);
  for (const version of supported) expect(supportedRuntime(version)).toBe(true);
  for (const version of [undefined, "", "18.10.2", "18.10.5", "18.11.0", "18.11.2", "18.11.4", "18.12.0", "19.0.0", "18.11.1-rc.1", "18.11.3-rc.1", "v18.11.3", "018.11.3", "18.11.3 junk", "18.11.3.4"]) {
    expect(supportedRuntime(version)).toBe(false);
  }
  expect(manifest.version).toBe(metadata.version);
  expect(supportedRuntime(metadata.version)).toBe(false);
});

test("unsupported or unidentifiable OMP fails before registering native handlers", () => {
  const previous = process.env.AOE_INSTANCE_ID;
  process.env.AOE_INSTANCE_ID = "compatibility-test";
  try {
    for (const version of [undefined, "18.10.2", "18.11.2", "19.0.0"]) {
      let registered = false;
      const api = { pi: { VERSION: version }, on() { registered = true; } };
      expect(() => initialize(api as Parameters<typeof initialize>[0])).toThrow("Install a supported OMP runtime or update");
      expect(registered).toBe(false);
    }
  } finally {
    if (previous === undefined) delete process.env.AOE_INSTANCE_ID;
    else process.env.AOE_INSTANCE_ID = previous;
  }
});

test("legacy native approval selector preserves cancellation and accepts only one explicit reply", async () => {
  for (const choice of ["Approve", "Deny"]) {
    let accept!: (choice: string) => boolean;
    let settled = false;
    let closed = false;
    const parent = new AbortController();
    const select = observeApprovalSelect((_title, _choices, options) => {
      const { promise, resolve } = Promise.withResolvers<string | undefined>();
      options?.signal?.addEventListener("abort", () => { settled = true; resolve(undefined); }, { once: true });
      return promise;
    }, submit => { accept = submit; }, () => { closed = true; });
    const result = select("Approve tool", ["Approve", "Deny"], { signal: parent.signal });
    expect(accept(choice)).toBe(true);
    expect(await result).toBe(choice);
    expect(settled).toBe(true);
    expect(closed).toBe(true);
    expect(parent.signal.aborted).toBe(false);
    expect(accept(choice)).toBe(false);
  }
  let accept!: (choice: string) => boolean;
  const select = observeApprovalSelect(async () => "Deny", submit => { accept = submit; }, () => {});
  expect(await select("Approve tool", ["Approve", "Deny"])).toBe("Deny");
  expect(accept("Approve")).toBe(false);
});

test("known runtime with a missing native API rejects startup before publishing", async () => {
  const previous = process.env.AOE_INSTANCE_ID;
  process.env.AOE_INSTANCE_ID = "compatibility-shape-test";
  try {
    for (const version of ["18.10.3", "18.10.4", "18.11.1", "18.11.3"]) {
      let start!: (_event: unknown, ctx: unknown) => Promise<void>;
      const api = {
        pi: { VERSION: version },
        on(event: string, handler: typeof start) { if (event === "session_start") start = handler; },
      };
      initialize(api as unknown as Parameters<typeof initialize>[0]);
      await expect(start({}, { sessionManager: {}, ui: {} })).rejects.toThrow("lacks the native session/select/input API");
    }
  } finally {
    if (previous === undefined) delete process.env.AOE_INSTANCE_ID;
    else process.env.AOE_INSTANCE_ID = previous;
  }
});

test("native approval selector recognizes session choices without changing native results", async () => {
  const sessionChoice = "Approve bash for this session";
  expect(isApprovalSelection(["Approve", "Deny"])).toBe(true);
  expect(isApprovalSelection(["Approve", sessionChoice, "Deny"])).toBe(true);
  for (const choices of [[], ["Approve"], ["Approve", "Cancel"], ["Other", sessionChoice, "Deny"], ["Approve", {}, "Deny"], ["Approve", "Other", "Deny"]]) {
    expect(isApprovalSelection(choices)).toBe(false);
  }
  for (const choice of ["Approve", sessionChoice, "Deny", undefined]) {
    let accept!: (choice: string) => boolean;
    const select = observeApprovalSelect(async () => choice, submit => { accept = submit; }, () => {});
    expect(await select("Approve tool", ["Approve", sessionChoice, "Deny"])).toBe(choice);
    expect(accept("Approve")).toBe(false);
  }
});
