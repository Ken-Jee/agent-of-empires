import initialize, { integrationDirectory } from "../omp-extension/index.ts";
import { readFileSync, statSync, writeFileSync } from "node:fs";
import path from "node:path";

// Driven by the Rust host regression over a real, isolated tmux pane; no tool runs.
const resultPath = process.argv[2]!;
const snapshotPath = path.join(integrationDirectory(process.getuid!(), process.env.AOE_INSTANCE_ID!),
  "integration-omp.agent-of-empires.json");
const handlers = new Map<string, (...args: unknown[]) => unknown>();
let input!: (data: string) => unknown;
const ui = {
  select(_title: string, _choices: string[], options?: { signal?: AbortSignal }) {
    return new Promise<string | undefined>(resolve => {
      options?.signal?.addEventListener("abort", () => resolve(undefined), { once: true });
    });
  },
  onTerminalInput(handler: typeof input) { input = handler; return () => {}; },
};
const waitFor = async (ready: () => boolean): Promise<void> => {
  const deadline = performance.now() + 10_000;
  while (!ready()) {
    if (performance.now() >= deadline) throw new Error("provider snapshot did not reach the expected state");
    await new Promise<void>(resolve => setImmediate(resolve));
  }
};

initialize({
  pi: { VERSION: "18.10.3" },
  on(event: string, handler: (...args: unknown[]) => unknown) { handlers.set(event, handler); },
} as unknown as Parameters<typeof initialize>[0]);
await handlers.get("session_start")!({}, {
  sessionManager: { getSessionId: () => "host-reply-session" },
  ui,
});
handlers.get("tool_approval_requested")!({
  sessionId: "host-reply-session", toolCallId: "approval-a", toolName: "bash",
});
void ui.select("Allow tool: bash", ["Approve", "Deny"]).then(selection => {
  writeFileSync(resultPath, selection ?? "cancelled");
});
await waitFor(() => JSON.parse(readFileSync(snapshotPath, "utf8")).prompt?.id === "approval-a");

process.stdin.setRawMode(true);
process.stdin.setEncoding("utf8");
let buffer = "";
process.stdin.on("data", async (data: string) => {
  buffer += data;
  while (buffer) {
    if (buffer.startsWith("s")) {
      buffer = buffer.slice(1);
      const previous = statSync(snapshotPath).ino;
      handlers.get("agent_start")!();
      // Acknowledge only after the extension publishes a snapshot following agent_start.
      await waitFor(() => statSync(snapshotPath).ino !== previous);
      writeFileSync(`${resultPath}.started`, "ready");
    } else if (buffer.startsWith("\u001b[24~")) {
      buffer = buffer.slice(5);
      input("\u001b[24~");
    } else if ("\u001b[24~".startsWith(buffer)) {
      break;
    } else {
      throw new Error(`unexpected fixture input: ${JSON.stringify(buffer)}`);
    }
  }
});
