/** 审计版在共享环境清洗器捕获 OTLP 配置前剔除遥测，防止 Host 再传给 Agent。 */
export function omitDesktopTelemetryEnvironment(
  env: Record<string, string | undefined>,
): Record<string, string> {
  return Object.fromEntries(
    Object.entries(env).filter(
      (entry): entry is [string, string] =>
        entry[1] !== undefined &&
        !/^(?:OTEL_|MODE_TELEMETRY_|MODE_MODEL_TELEMETRY_ENABLED$)/i.test(entry[0]),
    ),
  );
}
