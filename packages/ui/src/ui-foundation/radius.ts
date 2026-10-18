// 控件半径 → Tailwind 类的唯一映射（P1.1）。Button / Input / Select 共用，避免各写一份。
//
// 注意：这里映射的是 `resolveControlRadius(parent)` 的**结果档位**（lg/md/sm），
// 不是父容器的档位。父容器 none/xl/2xl 的结果是 lg → rounded-lg。

import { resolveControlRadius, type RadiusLevel } from "../lib/uiFoundation.js";

const CONTROL_RADIUS_CLASSES: Record<"lg" | "md" | "sm", string> = {
  lg: "rounded-lg",
  md: "rounded-md",
  sm: "rounded-sm",
};

export function controlRadiusClass(parentRadius: RadiusLevel = "xl"): string {
  return CONTROL_RADIUS_CLASSES[resolveControlRadius(parentRadius) as "lg" | "md" | "sm"];
}
