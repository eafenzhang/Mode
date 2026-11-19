// 报告形状与文本输出（spec §5/§3）。JSON 报告去掉 timing 字段后必须逐字节稳定。
import type { CaseResult, RunReport } from "./types.js";

export type ReportWithoutTiming = Omit<RunReport, "results"> & {
  results: Array<Omit<CaseResult, "durationMs">>;
};

export function buildReport(suite: string, results: readonly CaseResult[]): RunReport {
  const totals = { cases: results.length, passed: 0, failed: 0, recorded: 0 };
  for (const result of results) {
    totals[result.status] += 1;
  }
  return {
    schemaVersion: 1,
    suite,
    totals,
    results: results.map((result) => ({ ...result })),
  };
}

/** 确定性投影：剔除 durationMs 后的结构副本（测试与 spec §8.1 用）。 */
export function stripTiming(report: RunReport): ReportWithoutTiming {
  return {
    ...report,
    results: report.results.map(({ durationMs: _durationMs, ...rest }) => rest),
  };
}

export function formatReportText(report: RunReport): string {
  const lines: string[] = [];
  lines.push(
    `suite: ${report.suite} — ${report.totals.passed} passed, ` +
      `${report.totals.failed} failed, ${report.totals.recorded} recorded ` +
      `(${report.totals.cases} cases)`,
  );
  for (const result of report.results) {
    lines.push(
      `  [${result.status}] ${result.case} ` +
        `(${result.durationMs}ms, ${result.turns} turn(s), tools: ` +
        `${result.toolCalls.length > 0 ? result.toolCalls.join(", ") : "none"})`,
    );
    for (const mismatch of result.mismatches.slice(0, 5)) {
      lines.push(`    - ${mismatch.path}: expected ${JSON.stringify(mismatch.expected)}, got ${JSON.stringify(mismatch.actual)}`);
    }
    if (result.mismatches.length > 5) {
      lines.push(`    - … ${result.mismatches.length - 5} more mismatch(es)`);
    }
    if (result.error) {
      lines.push(`    - error: ${result.error}`);
    }
    if (result.stderr) {
      lines.push(`    - stderr: ${result.stderr.slice(0, 500)}`);
    }
  }
  return `${lines.join("\n")}\n`;
}
