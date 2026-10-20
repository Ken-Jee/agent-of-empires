import type { Workspace } from "../../lib/types";
import { setSessionColor, setSessionNotifications, type RuntimeCursor, type SessionMutation } from "../../lib/api";
import { useIdleDecayWindowMs } from "../../lib/idleDecay";
import { useUnreadIndicatorEnabled } from "../../lib/unreadIndicator";
import { usePluginAttention } from "../../lib/pluginUiContext";
import type { OptimisticTriage } from "../../lib/sidebarOptimistic";
import { reportError } from "../../lib/toastBus";
import { deriveRowModel, SESSION_COLOR_OPTIONS, type RowModel } from "./rowModel";
import { usePendingSetting } from "./usePendingSetting";

export interface RowObservation {
  observedById: Record<string, RuntimeCursor>;
  runtimeEpoch: string | null;
  onSessionMutation: (mutation: SessionMutation) => void;
}

/** The row model with its notify and color picks applied optimistically. */
export function useSessionRowModel(
  workspace: Workspace,
  optimistic: OptimisticTriage,
  isActive: boolean,
  observation: RowObservation,
) {
  const idleDecayWindowMs = useIdleDecayWindowMs();
  const unreadIndicatorEnabled = useUnreadIndicatorEnabled();
  const pluginAttention = usePluginAttention();
  const derived = deriveRowModel(workspace, optimistic, {
    idleDecayWindowMs,
    isActive,
    unreadIndicatorEnabled,
    pluginAttention,
  });
  const { sessionId } = derived;
  const settingIdentity = `${workspace.id}:${observation.runtimeEpoch ?? ""}:${workspace.sessions
    .map((session) => session.id)
    .sort()
    .join(",")}`;
  const [notifyPreset, setNotify] = usePendingSetting(
    derived.notifyPreset,
    async (preset) => {
      if (!sessionId) return null;
      const mutation = await setSessionNotifications(sessionId, preset);
      if (!mutation) return null;
      observation.onSessionMutation(mutation);
      return [{ id: sessionId, cursor: mutation.cursor }];
    },
    () => reportError("Could not change notifications. Please try again."),
    observation.observedById,
    settingIdentity,
  );
  const [sessionColor, setColor] = usePendingSetting(
    derived.sessionColor,
    async (color) => {
      const results = await Promise.all(
        workspace.sessions.map(async ({ id }) => {
          const mutation = await setSessionColor(id, color);
          if (mutation) observation.onSessionMutation(mutation);
          return mutation ? { id, cursor: mutation.cursor } : null;
        }),
      );
      return results.every((receipt) => receipt !== null) ? results : null;
    },
    () => reportError("Could not change the session color. Please try again."),
    observation.observedById,
    settingIdentity,
  );
  const model: RowModel = {
    ...derived,
    notifyPreset,
    sessionColor,
    sessionColorDot: SESSION_COLOR_OPTIONS.find((o) => o.key === sessionColor)?.dotClass ?? null,
  };
  return { model, setNotify, setColor };
}
