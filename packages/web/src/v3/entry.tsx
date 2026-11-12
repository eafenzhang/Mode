import { createRoot } from "react-dom/client";
import "@mode/ui/styles.css";
import { applyWebThemeToDocument } from "../webThemeSeed.js";
import { WebV3Page } from "./WebV3Page.js";

// rc.mymode.cloud 的独立轻量入口：只装远程控制所需模块，不引入主应用 shell。
// 主应用入口（main.tsx）仍按 `/v3` 路径提供同一页面，二者共用 WebV3Page。
document.title = "Mode - 远程控制";
applyWebThemeToDocument();

createRoot(document.getElementById("root")!).render(<WebV3Page />);
