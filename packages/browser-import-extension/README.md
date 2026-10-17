# Mode Browser Import（Chrome MV3 扩展，P1）

把当前 Chrome profile 的 cookie（含 app-bound v20）经 Native Messaging 交给 Mode。
规格：`docs/specs/chrome-browser-import-extension.md`。

## 构建

```bash
pnpm --filter @mode/browser-import-extension build
```

产物在 `dist/`（gitignored）：`manifest.json`、`popup.html`、`popup.js`。

## 本地加载（开发）

1. Chrome → `chrome://extensions` → 开启开发者模式；
2. 「加载已解压的扩展程序」→ 选择 `packages/browser-import-extension/dist`；
3. 需要先注册 Native host（T5）：
   `HKCU\Software\Google\Chrome\NativeMessagingHosts\com.mode.browser_import`
   指向 host manifest，`allowed_origins` 含本扩展的 ID。

## 边界

- 只在用户点击 popup 时读取 cookie；没有后台读取、没有网络上传。
- 单条 Native Messaging 消息上限 1 MiB；超过显示 `payload_too_large`（分片在 T6 之后评估）。
- 分发方式（Web Store / 企业策略 / 侧载）仍是开放决策。
