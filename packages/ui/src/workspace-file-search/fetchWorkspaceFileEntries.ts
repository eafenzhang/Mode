import type { IFileService } from "@mode/services";
import { WORKSPACE_FILE_ENTRIES_CHUNK_SIZE } from "@mode/shared/workspaceFileEntriesCodec";

/**
 * 分块拉取 workspace 文件索引的列式 packed 字符串。
 * 单条大消息在远端共享通道上是队头阻塞源：远端工作区全部 RPC 走同一条连接，
 * 大帧把后续小帧挡在队列里（环回实测 4×4MB 批量下小 RPC p95 98ms，真实链路按
 * 带宽放大）；renderer 接收端的分帧重组还是秒级主线程长任务（实测 4.6-6.3s），
 * 输入会冻结。分块（512K 字符/块，对齐 attachmentChunkMaxBytes 口径）+ 块间
 * setTimeout(0) 让出事件循环后，主线程每次只处理一小块（~6ms），键入事件始终能插队。
 * 整体耗时（字节 ÷ 带宽 + 块数 × RTT）基本不变，但 UI 与周边 RPC 不再被单帧占线。
 * 依据 docs/specs/remote-workspace-rpc-bulk-chunking.md。
 */
export async function fetchWorkspaceFileEntriesPacked(
  fileService: Pick<IFileService, "listWorkspaceFilesLength" | "listWorkspaceFilesRange">,
  rootPath: string,
): Promise<string> {
  const totalLength = await fileService.listWorkspaceFilesLength({ rootPath });
  let packed = "";
  for (let offset = 0; offset < totalLength; offset += WORKSPACE_FILE_ENTRIES_CHUNK_SIZE) {
    const chunk = await fileService.listWorkspaceFilesRange({
      rootPath,
      offset,
      length: WORKSPACE_FILE_ENTRIES_CHUNK_SIZE,
    });
    packed += chunk;
    // 块间让出：处理下一块前先放行排队的输入/渲染任务。
    await new Promise<void>((resolve) => setTimeout(resolve, 0));
  }
  return packed;
}
