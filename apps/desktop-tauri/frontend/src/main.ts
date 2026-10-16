/**
 * 骨架前端入口。
 *
 * 装配顺序：拿 Tauri 全局 API → 建平台服务 → 渲染验收台。
 * 两步都失败即报错（不静默出白屏）：骨架期的白屏最难排查，明确报错才能定位是
 * `withGlobalTauri` 没开、还是不是由 Tauri webview 加载的。
 */
import "./styles.css";
import { renderHarness } from "./harness.js";
import { renderHostChannelPanel } from "./hostChannel.js";
import { createPlatform } from "./platform.js";

const root = document.getElementById("root");
if (!root) {
  throw new Error("找不到 #root：index.html 与 main.ts 不匹配");
}

try {
  renderHarness(root, createPlatform());
  // M3 Task 1：把「端口 + database-startup 状态流」到没到页面印出来——这是渲染层启动闸门的
  // 两条输入，也是本页唯一能肉眼验证 Host 通道的地方。
  renderHostChannelPanel(root);
} catch (error) {
  // 启动失败必须看得见（骨架期没有日志通道，直接写进页面）。
  root.replaceChildren();
  const failure = document.createElement("pre");
  failure.className = "result result-error";
  failure.textContent = `骨架前端启动失败：\n${
    error instanceof Error ? `${error.name}: ${error.message}` : String(error)
  }`;
  root.append(failure);
}
