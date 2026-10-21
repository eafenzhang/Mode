// 统一构建的 Web 入口：由 web.html 加载，副作用模块就是 packages/web 的浏览器引导。
// 放在独立文件里是为了让 desktop 的 tsconfig 只看到一条模块声明，不把 web 源码
// 拉进 rootDir=src/renderer 的工程（web 源码由 @mode/web 自己的检查覆盖）。
import "@mode/web-entry";
