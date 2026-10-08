import { app, session, type Session } from "electron";
import { shouldBlockOfficialPlatformUrl } from "@mode/shared";

/** 审计版不连接官方平台：覆盖历史缓存图片、webview、重定向和 Electron net 请求。 */
export function installOfficialPlatformNetworkPolicy(): void {
  const install = (target: Session) => {
    target.webRequest.onBeforeRequest((details, callback) => {
      callback({ cancel: shouldBlockOfficialPlatformUrl(details.url) });
    });
  };
  app.on("session-created", install);
  // 注册必须先于 app ready；defaultSession 也可能已由其它初始化代码创建。
  void app.whenReady().then(() => install(session.defaultSession));
}
