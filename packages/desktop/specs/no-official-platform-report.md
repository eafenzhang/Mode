# ZCodium 官方平台断连报告

本轮已禁用应用自带的官方平台服务入口，并保留用户配置的模型 Provider、本地功能和 GitHub Releases 更新。不提交 Git commit，不删除历史凭证或用户配置。

## 各类连接的处理

| 类别          | 处理                                                                                                                                                                                                                  |
| ------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| OAuth         | Desktop 返回未登录和空 Provider 列表；登录、刷新、回调、轮询及二次业务 token 兑换在副作用前短路。登出只走本地清理，不通知平台。Web/CLI 登录和 token 兑换也禁用。                                                      |
| 对话分享      | 发布、预检、导入和 HTTP 客户端入口均拒绝，提示 ZCodium 中不可用。Web 分享预览禁用。官方分享 URL 构造保留兼容结构，但无法进入网络路径。                                                                                |
| 反馈          | 提交和 HTTP 请求在请求前报不可用，错误包含 GitHub Issues 地址；帮助配置直接返回 https://github.com/eafenzhang/ZCodium/issues。保留本地日志导出。                                                                 |
| 套餐/额度     | 套餐统一请求入口、BigModel 权益与团队 API Key、账户 Provider API、套餐可用性查询、Start Plan 余额、用量与额度查询均短路。                                                                                             |
| 官方 MCP      | 凭证解析直接返回 official_auth_unavailable，不读取或发放官方凭证。                                                                                                                                                    |
| offPeak       | 闲时任务凭证和 ticket HTTP 请求在副作用前拒绝，无法启动官方模型网关调用。                                                                                                                                             |
| 启动/远端配置 | 启动 context prompt 预热返回空配置；客户端商店排序为空、本地场景列表为空、帮助配置本地化；内置 Provider 配置下载返回 null，保留已有本地配置。设备 MID 代码经检查只在本地持久化，本轮未发现独立官方设备注册/心跳请求。 |
| CDN/插件      | 移除默认官方远端市场；保留本地内置插件播种和个人来源。推荐图标使用本地资源，内置插件定义使用内嵌图标，历史官方图片 URL 在 UI 渲染前被拒绝。Desktop CDN 自动下载返回空来源。                                           |
| 出口保护      | Node ApiClient、CLI HTTP adapter、CLI 模型 fetch 在网络调用前拒绝官方平台域名及其子域；Electron 对已创建的默认 Session 与后续 Session 注册请求拦截，覆盖图片、webview、缓存 URL 和重定向。                            |
| 用户模型      | 删除将 BigModel/Z.ai 模型 URL 改写到官方平台网关的行为；保持原 URL、Request、请求体和鉴权头。api.z.ai/open.bigmodel.cn 等用户自配模型地址不在平台域名黑名单内。                                                       |

共享禁用策略不能被环境变量、已保存 endpoint 或登录缓存重新开启。保留接口处添加了中文审计说明。业务状态所有者、Host/lease 和桌面/手机会话交付语义不变。

## 验证

- 根 `pnpm typecheck`：分阶段及最终检查均退出 0。
- 根 `pnpm lint`：分阶段及最终检查均退出 0；52 warnings、0 errors，未将警告表述为全部清零。
- CLI adapters：`tsc --noEmit -p apps/mode-cli/packages/adapters/tsconfig.json` 退出 0。
- Desktop 全部回归 18/18 通过：新增断连测试 10 项、原 no-telemetry 4 项、GitHub Releases 更新 4 项。包括源码 grep 风格扫描、运行实际方法体确认副作用前短路、CLI OAuth 网络替身、Electron Session 拦截、历史内置模型 endpoint 拦截及用户模型请求参数保留。
- 架构检查：0 violations、0 baseline、0 new；`git diff --check` 通过。
- 额外 Electron main 类型检查未通过：75 项错误。使用当前 HEAD 源码覆盖编译器读取并比对诊断，基线同为 75 项，归一化联合类型显示顺序后新增 0 项。根 typecheck 本身不包含这个子工程；本轮未扩展修改这些无关错误。

工具环境：仓库指定 Node 24.14.0 / pnpm 10.33.2；本机为 Node 24.16.0，默认 pnpm 11.8.0 自动下载指定版本被网络限制阻断。实际通过的检查使用本机已有 pnpm 10.18.0：

```sh
node /Users/daiqiang/.cache/node/corepack/v1/pnpm/10.18.0/bin/pnpm.cjs --manage-package-manager-versions=false typecheck
node /Users/daiqiang/.cache/node/corepack/v1/pnpm/10.18.0/bin/pnpm.cjs --manage-package-manager-versions=false lint
node --test packages/desktop/tests/*.test.mjs
node scripts/architecture/architecture-check.mjs check --changed
```

## 边界与未验证项

- 未发现仍可主动调用官方平台的内置服务路径。`config/provider/mode-builtin.json` 的历史平台模型 URL 和共享 endpoint 常量仍保留用于结构兼容，实际发送会被拒绝，回归测试覆盖这些配置 URL。
- 用户主动打开系统浏览器的文档链接、用户自配模型 API 按需求保留，不计为平台后台连接。用户自行安装的插件或通过终端执行的任意程序不等于应用内置平台调用，本轮未改写这些外部程序。
- 没有执行完整 Electron 应用 E2E 或真实启动抓包；网络零调用证明来自方法体执行、网络替身和请求拦截测试，不宣称已有打包应用的实机抓包证明。
- 工作区 freshness 脚本已运行，但 git fetch 因无法写入 `.git/FETCH_HEAD` 失败；无法证明当前检出与远端最新提交一致。

## 完整改动文件

- [apps/mode-cli/packages/adapters/src/auth/bigmodel-oauth.ts](../../../apps/mode-cli/packages/adapters/src/auth/bigmodel-oauth.ts)
- [apps/mode-cli/packages/adapters/src/auth/cli-oauth.ts](../../../apps/mode-cli/packages/adapters/src/auth/cli-oauth.ts)
- [apps/mode-cli/packages/adapters/src/auth/coding-plan-api-key.ts](../../../apps/mode-cli/packages/adapters/src/auth/coding-plan-api-key.ts)
- [apps/mode-cli/packages/adapters/src/http/index.ts](../../../apps/mode-cli/packages/adapters/src/http/index.ts)
- [apps/mode-cli/packages/adapters/src/model/official-coding-plan-gateway.ts](../../../apps/mode-cli/packages/adapters/src/model/official-coding-plan-gateway.ts)
- [apps/mode-cli/packages/bootstrap/src/app/official-plugin-definitions.ts](../../../apps/mode-cli/packages/bootstrap/src/app/official-plugin-definitions.ts)
- [packages/desktop/specs/no-official-platform-report.md](../../../packages/desktop/specs/no-official-platform-report.md)
- [packages/desktop/specs/no-official-platform.md](../../../packages/desktop/specs/no-official-platform.md)
- [packages/desktop/src/main/desktopContextPromptRollout.ts](../../../packages/desktop/src/main/desktopContextPromptRollout.ts)
- [packages/desktop/src/main/desktopOfficialPlatformPolicy.ts](../../../packages/desktop/src/main/desktopOfficialPlatformPolicy.ts)
- [packages/desktop/src/main/index.ts](../../../packages/desktop/src/main/index.ts)
- [packages/desktop/src/main/remoteCdn.ts](../../../packages/desktop/src/main/remoteCdn.ts)
- [packages/desktop/tests/no-official-platform.test.mjs](../../../packages/desktop/tests/no-official-platform.test.mjs)
- [packages/provider-node/src/mode-builtin-download.ts](../../../packages/provider-node/src/mode-builtin-download.ts)
- [packages/services/src/bigmodel/codingPlanEntitlement.ts](../../../packages/services/src/bigmodel/codingPlanEntitlement.ts)
- [packages/services/src/bigmodel/teamPlanApiKey.ts](../../../packages/services/src/bigmodel/teamPlanApiKey.ts)
- [packages/services/src/client-config/clientConfigService.ts](../../../packages/services/src/client-config/clientConfigService.ts)
- [packages/services/src/client-scenes/clientScenesService.ts](../../../packages/services/src/client-scenes/clientScenesService.ts)
- [packages/services/src/coding-plan-subscription/bigmodelCodingPlanSubscriptionProvider.ts](../../../packages/services/src/coding-plan-subscription/bigmodelCodingPlanSubscriptionProvider.ts)
- [packages/services/src/conversation-share/conversationShareHttpClient.ts](../../../packages/services/src/conversation-share/conversationShareHttpClient.ts)
- [packages/services/src/conversation-share/conversationShareService.ts](../../../packages/services/src/conversation-share/conversationShareService.ts)
- [packages/services/src/feedback/feedbackHttpClient.ts](../../../packages/services/src/feedback/feedbackHttpClient.ts)
- [packages/services/src/feedback/feedbackService.ts](../../../packages/services/src/feedback/feedbackService.ts)
- [packages/services/src/model-provider/accountProviderApiClient.ts](../../../packages/services/src/model-provider/accountProviderApiClient.ts)
- [packages/services/src/model-provider/accountProviderTeamPlanRequestKey.ts](../../../packages/services/src/model-provider/accountProviderTeamPlanRequestKey.ts)
- [packages/services/src/model-provider/codingPlanProviderAvailability.ts](../../../packages/services/src/model-provider/codingPlanProviderAvailability.ts)
- [packages/services/src/model-provider/legacyTeamOrganizationResolver.ts](../../../packages/services/src/model-provider/legacyTeamOrganizationResolver.ts)
- [packages/services/src/model-provider/zaiStartPlanBilling.ts](../../../packages/services/src/model-provider/zaiStartPlanBilling.ts)
- [packages/services/src/oauth/oauthService.ts](../../../packages/services/src/oauth/oauthService.ts)
- [packages/services/src/oauth/providers/bigmodelProviderAdapter.ts](../../../packages/services/src/oauth/providers/bigmodelProviderAdapter.ts)
- [packages/services/src/oauth/providers/bigmodelProviderConfig.ts](../../../packages/services/src/oauth/providers/bigmodelProviderConfig.ts)
- [packages/services/src/oauth/providers/zaiProviderAdapter.ts](../../../packages/services/src/oauth/providers/zaiProviderAdapter.ts)
- [packages/services/src/oauth/providers/zaiProviderConfig.ts](../../../packages/services/src/oauth/providers/zaiProviderConfig.ts)
- [packages/services/src/official-mcp/officialMcpCredentials.ts](../../../packages/services/src/official-mcp/officialMcpCredentials.ts)
- [packages/services/src/providers/api/nodeApiClient.ts](../../../packages/services/src/providers/api/nodeApiClient.ts)
- [packages/services/src/session/offPeakRuntimeModel.ts](../../../packages/services/src/session/offPeakRuntimeModel.ts)
- [packages/services/src/session/offPeakServerClient.ts](../../../packages/services/src/session/offPeakServerClient.ts)
- [packages/services/src/usage-stats/providers/bigmodelUsageQuotaProvider.ts](../../../packages/services/src/usage-stats/providers/bigmodelUsageQuotaProvider.ts)
- [packages/services/src/usage-stats/providers/modeMcpQuotaProvider.ts](../../../packages/services/src/usage-stats/providers/modeMcpQuotaProvider.ts)
- [packages/shared/src/helpAppConfig.ts](../../../packages/shared/src/helpAppConfig.ts)
- [packages/shared/src/index.ts](../../../packages/shared/src/index.ts)
- [packages/shared/src/officialPlatformPolicy.ts](../../../packages/shared/src/officialPlatformPolicy.ts)
- [packages/shared/src/plugin-marketplaces.ts](../../../packages/shared/src/plugin-marketplaces.ts)
- [packages/ui/src/lib/trustedImageUrl.ts](../../../packages/ui/src/lib/trustedImageUrl.ts)
- [packages/ui/src/v4/featureSuggestedPrompts.ts](../../../packages/ui/src/v4/featureSuggestedPrompts.ts)
- [packages/web/src/auth/zaiWebOAuthProvider.ts](../../../packages/web/src/auth/zaiWebOAuthProvider.ts)
- [packages/web/src/share/conversationSharePreviewClient.ts](../../../packages/web/src/share/conversationSharePreviewClient.ts)
