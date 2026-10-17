import { app, ipcMain } from "electron";
import { PlatformChannels, type ChromeBrowserDataImportOptions } from "@mode/shared";
import { clearEmbeddedBrowserData, importChromeBrowserData } from "./browserDataManager.js";
import { closeActiveBrowserImportChannels } from "./browserImportExtension.js";

export function registerBrowserDataIpcHandlers(logger: {
  info: (...args: unknown[]) => void;
  warn: (...args: unknown[]) => void;
}) {
  // 退出时清掉仍在等待扩展的通道，避免 channel.json 指向已消失的命名管道。
  app.once("will-quit", () => {
    closeActiveBrowserImportChannels();
  });
  ipcMain.handle(PlatformChannels.ImportChromeBrowserData, (_event, value: unknown) => {
    const requested = value as ChromeBrowserDataImportOptions | undefined;
    // renderer 只能为本次调用显式传 true；main 不接受或持久化其他授权形态。
    const allowElevatedChromeDecryption = requested?.allowElevatedChromeDecryption === true;
    // P1：显式开关才走扩展通道；默认仍是现有 helper 路径（T5/T6 后再切默认）。
    const browserImportExtension =
      requested?.browserImportExtension === true ||
      process.env.MODE_BROWSER_IMPORT_EXTENSION === "1";
    return importChromeBrowserData({
      allowElevatedChromeDecryption,
      browserImportExtension,
      logger,
    });
  });
  ipcMain.handle(PlatformChannels.ClearEmbeddedBrowserData, (_event, mode: unknown) => {
    if (mode !== "cache" && mode !== "all") {
      return { success: false, error: "invalid_clear_mode" };
    }
    return clearEmbeddedBrowserData({ logger, mode });
  });
}
