// 随包插件 seed 标记文件名（spec: p3-tutto-naming S4）。
export const OFFICIAL_PLUGIN_SEED_MARKER_FILE = ".tutto-plugin-seed.json";
export const LEGACY_OFFICIAL_PLUGIN_SEED_MARKER_FILES = [".zcode-plugin-seed.json"] as const;

export function resolveOfficialPluginSeedMarkerFileName(
  exists: (fileName: string) => boolean,
): string {
  if (exists(OFFICIAL_PLUGIN_SEED_MARKER_FILE)) {
    return OFFICIAL_PLUGIN_SEED_MARKER_FILE;
  }
  return (
    LEGACY_OFFICIAL_PLUGIN_SEED_MARKER_FILES.find((fileName) => exists(fileName)) ??
    OFFICIAL_PLUGIN_SEED_MARKER_FILE
  );
}
