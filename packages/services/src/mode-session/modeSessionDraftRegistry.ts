import type { ModeSessionStateSnapshot } from "@mode/shared";
import type {
  ModeSessionWorkspaceTarget,
  ModeTaskTarget,
} from "#src/mode-session/modeSession.js";

function getWorkspaceKey(target: ModeSessionWorkspaceTarget): string {
  return target.workspaceIdentity?.trim() || target.workspacePath;
}

function getSessionScopedKey(target: ModeTaskTarget): string {
  return `${getWorkspaceKey(target)}\0${target.sessionId}`;
}

export function createModeDeferredDraftRegistry() {
  const sessionKeys = new Set<string>();

  return {
    remember(params: ModeSessionWorkspaceTarget, snapshot: ModeSessionStateSnapshot): void {
      sessionKeys.add(
        getSessionScopedKey({
          workspacePath: snapshot.session.workspace.workspacePath,
          workspaceIdentity:
            snapshot.session.workspace.workspaceIdentity ?? params.workspaceIdentity,
          sessionId: snapshot.session.sessionId,
        }),
      );
    },

    has(target: ModeTaskTarget): boolean {
      return sessionKeys.has(getSessionScopedKey(target));
    },

    forget(target: ModeTaskTarget): void {
      sessionKeys.delete(getSessionScopedKey(target));
    },
  };
}
