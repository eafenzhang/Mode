// P1.1 Chart 组合件（spec: docs/specs/m4-ui-foundation.md §2.13）。
// 只负责 config→CSS 变量、容器与 tooltip/legend 的展示；数据与图表类型由调用方用 recharts 组合。

import {
  createContext,
  useContext,
  useId,
  useMemo,
  type ComponentProps,
  type ComponentType,
  type ReactNode,
} from "react";
import * as RechartsPrimitive from "recharts";

export interface ChartConfigEntry {
  label?: ReactNode;
  icon?: ComponentType<{ className?: string }>;
  color?: string;
}

export type ChartConfig = Record<string, ChartConfigEntry>;

interface ChartContextValue {
  config: ChartConfig;
}

const ChartContext = createContext<ChartContextValue | null>(null);

export function useChart(): ChartContextValue {
  const context = useContext(ChartContext);
  if (!context) {
    throw new Error("useChart must be used within a <ChartContainer />");
  }
  return context;
}

export interface ChartContainerProps extends ComponentProps<"div"> {
  config: ChartConfig;
  id?: string;
  /** 关闭 ResponsiveContainer（静态渲染/测试用）。 */
  responsive?: boolean;
}

export function ChartContainer({
  id,
  className,
  children,
  config,
  responsive = true,
  ...rest
}: ChartContainerProps): ReactNode {
  const uniqueId = useId();
  const chartId = `chart-${id ?? uniqueId.replace(/:/g, "")}`;
  const value = useMemo(() => ({ config }), [config]);
  return (
    <ChartContext.Provider value={value}>
      <div
        {...rest}
        data-chart={chartId}
        data-slot="chart"
        className={["flex justify-center text-ui-xs", className].filter(Boolean).join(" ")}
      >
        <ChartStyle id={chartId} config={config} />
        {responsive ? (
          <RechartsPrimitive.ResponsiveContainer>{children}</RechartsPrimitive.ResponsiveContainer>
        ) : (
          children
        )}
      </div>
    </ChartContext.Provider>
  );
}

export function ChartStyle({ id, config }: { id: string; config: ChartConfig }): ReactNode {
  const colorConfig = Object.entries(config).filter(([, item]) => item.color);
  if (colorConfig.length === 0) {
    return null;
  }
  const css = colorConfig
    .map(([key, item]) => `[data-chart="${id}"] { --color-${key}: ${item.color}; }`)
    .join("\n");
  return <style dangerouslySetInnerHTML={{ __html: css }} />;
}

export const ChartTooltip = RechartsPrimitive.Tooltip;

export interface ChartTooltipPayloadItem {
  name?: string | number;
  dataKey?: string | number;
  value?: string | number;
  color?: string;
  payload?: Record<string, unknown>;
  [key: string]: unknown;
}

export interface ChartTooltipContentProps extends Omit<ComponentProps<"div">, "content"> {
  active?: boolean;
  payload?: ReadonlyArray<ChartTooltipPayloadItem>;
  label?: unknown;
  hideLabel?: boolean;
  hideIndicator?: boolean;
  indicator?: "line" | "dot" | "dashed";
  nameKey?: string;
  labelKey?: string;
  labelFormatter?: (label: unknown, payload: ReadonlyArray<ChartTooltipPayloadItem>) => ReactNode;
  formatter?: (
    value: unknown,
    name: unknown,
    item: ChartTooltipPayloadItem,
    index: number,
    payload: unknown,
  ) => ReactNode;
}

function indicatorColor(
  item: ChartTooltipPayloadItem,
  config: ChartConfigEntry | undefined,
): string {
  return item.color ?? config?.color ?? "var(--color-border)";
}

export function ChartTooltipContent({
  active,
  payload,
  label,
  hideLabel = false,
  hideIndicator = false,
  indicator = "dot",
  nameKey,
  labelKey,
  labelFormatter,
  formatter,
  className,
  children,
  ...rest
}: ChartTooltipContentProps): ReactNode {
  const { config } = useChart();
  const items = payload ?? [];
  if (!active || items.length === 0) {
    return null;
  }
  const configKey = (item: ChartTooltipPayloadItem): string => {
    const key = nameKey ?? item.dataKey ?? item.name;
    return key === undefined ? "" : String(key);
  };
  const resolvedLabel =
    labelKey && typeof label === "object" && label !== null
      ? (label as Record<string, unknown>)[labelKey]
      : label;
  return (
    <div
      {...rest}
      data-slot="chart-tooltip"
      className={[
        "grid min-w-32 gap-1.5 rounded-lg border border-popover-border bg-popover px-2.5 py-1.5 shadow-md",
        className,
      ]
        .filter(Boolean)
        .join(" ")}
    >
      {!hideLabel && resolvedLabel !== undefined && resolvedLabel !== null ? (
        <div className="font-medium text-foreground">
          {labelFormatter ? labelFormatter(resolvedLabel, items) : String(resolvedLabel)}
        </div>
      ) : null}
      <div className="grid gap-1.5">
        {items.map((item, index) => {
          const itemConfig = config[configKey(item)];
          const name = itemConfig?.label ?? item.name;
          return (
            <div
              key={`${configKey(item)}-${String(item.value)}-${index}`}
              className="flex w-full items-center gap-2"
            >
              {!hideIndicator ? (
                <span
                  aria-hidden="true"
                  className={[
                    "shrink-0",
                    indicator === "line"
                      ? "h-2.5 w-1 rounded-full"
                      : indicator === "dashed"
                        ? "h-0 w-2.5 border-t-2 border-dashed"
                        : "size-2 rounded-[2px]",
                  ].join(" ")}
                  style={
                    indicator === "dashed"
                      ? { borderColor: indicatorColor(item, itemConfig) }
                      : { backgroundColor: indicatorColor(item, itemConfig) }
                  }
                />
              ) : null}
              {name !== undefined ? (
                <span className="min-w-0 flex-1 truncate text-foreground-subtle">{name}</span>
              ) : null}
              {formatter ? (
                formatter(item.value, item.name, item, index, item.payload)
              ) : (
                <span className="font-mono font-medium tabular-nums text-foreground">
                  {typeof item.value === "number"
                    ? item.value.toLocaleString()
                    : String(item.value)}
                </span>
              )}
            </div>
          );
        })}
      </div>
      {children}
    </div>
  );
}

export const ChartLegend = RechartsPrimitive.Legend;

export interface ChartLegendPayloadItem {
  value?: string | number;
  dataKey?: string | number;
  color?: string;
  [key: string]: unknown;
}

export interface ChartLegendContentProps extends Omit<ComponentProps<"div">, "content"> {
  payload?: ReadonlyArray<ChartLegendPayloadItem>;
  verticalAlign?: "top" | "bottom";
  hideIcon?: boolean;
  nameKey?: string;
}

export function ChartLegendContent({
  payload,
  verticalAlign = "bottom",
  hideIcon = false,
  nameKey,
  className,
  ...rest
}: ChartLegendContentProps): ReactNode {
  const { config } = useChart();
  const items = payload ?? [];
  if (items.length === 0) {
    return null;
  }
  return (
    <div
      {...rest}
      data-slot="chart-legend"
      className={[
        "flex items-center justify-center gap-4",
        verticalAlign === "top" ? "pb-3" : "pt-3",
        className,
      ]
        .filter(Boolean)
        .join(" ")}
    >
      {items.map((item, index) => {
        const key = nameKey ?? item.dataKey ?? item.value;
        const itemConfig = config[key === undefined ? "" : String(key)];
        const Icon = itemConfig?.icon;
        return (
          <div key={`${String(key)}-${index}`} className="flex items-center gap-1.5">
            {!hideIcon ? (
              Icon ? (
                <Icon className="size-2.5" />
              ) : (
                <span
                  aria-hidden="true"
                  className="size-2 shrink-0 rounded-[2px]"
                  style={{ backgroundColor: item.color ?? itemConfig?.color }}
                />
              )
            ) : null}
            <span className="text-ui-xs text-foreground-subtle">
              {itemConfig?.label ?? item.value}
            </span>
          </div>
        );
      })}
    </div>
  );
}
