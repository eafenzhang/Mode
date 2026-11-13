import { useMemo } from "react";
import {
  resolveWorkspaceOpenInEditorTarget,
  type WorkspaceOpenInEditorScope,
} from "@/lib/workspaceOpenInEditorTarget.js";
import { useOptionalTabStore } from "@/store/TabStoreProvider.js";

export function useWorkspaceOpenInEditorTarget(scope: WorkspaceOpenInEditorScope) {
  const tabs = useOptionalTabStore((state) => state.tabs);

  return useMemo(
    () => resolveWorkspaceOpenInEditorTarget(tabs, scope),
    [scope.workspaceIdentity, scope.workspacePath, scope.workspaceRemoteSessionId, tabs],
  );
}
