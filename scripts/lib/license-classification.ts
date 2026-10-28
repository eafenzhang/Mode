// 许可证表达式分类的唯一实现。
//
// 组合表达式必须按语义计算：OR 取最宽松（任选其一），AND 取最严格（全部满足）。
// 旧实现用 `or[0] || and[0]` 兜底，会把 `MIT AND GPL-3.0` 误判为 green；这里不再保留
// 单成员兜底，未知表达式落到 review，让 check 失败而不是静默放行。

const GREEN =
  /^(MIT|MIT-0|ISC|BSD-2-Clause|BSD-3-Clause|BSD-4-Clause|0BSD|Unlicense|Apache-2\.0|Zlib|WTFPL|Artistic-2\.0|BlueOak-1\.0\.0|CC0-1\.0|CC-BY-4\.0|CC-BY-3\.0|BSD|Python-2\.0)$/i;

// 低 → 高。AND 取最大，OR 取最小；unresolved 必须比所有已知许可证更严格。
const SEVERITY = new Map([
  ["green", 0],
  ["yellow-cc", 1],
  ["yellow-weak", 2],
  ["yellow-lgpl", 3],
  ["review", 4],
  ["red-nc", 5],
  ["red-semiopen", 6],
  ["red-gpl", 7],
  ["red-agpl", 8],
  ["unresolved", 9],
]);

export function licenseSeverity(bucket) {
  return SEVERITY.get(bucket) ?? SEVERITY.get("review");
}

function normalizeExpression(raw) {
  if (Array.isArray(raw)) return raw.map(normalizeExpression).filter(Boolean).join(" OR ");
  if (typeof raw !== "string") return "";
  return raw.replace(/\s+/g, " ").trim();
}

function isBalanced(expression) {
  let depth = 0;
  for (const char of expression) {
    if (char === "(") depth += 1;
    else if (char === ")") {
      depth -= 1;
      if (depth < 0) return false;
    }
  }
  return depth === 0;
}

function stripOuterParens(expression) {
  let current = expression.trim();
  for (;;) {
    if (!current.startsWith("(") || !current.endsWith(")")) return current;
    let depth = 0;
    let wrapsWhole = true;
    for (let index = 0; index < current.length; index += 1) {
      const char = current[index];
      if (char === "(") depth += 1;
      if (char === ")") depth -= 1;
      if (depth === 0 && index < current.length - 1) {
        wrapsWhole = false;
        break;
      }
    }
    if (!wrapsWhole) return current;
    current = current.slice(1, -1).trim();
  }
}

/** 只在括号外按完整词拆分；`AND`/`OR` 两侧必须有空白，避免误伤许可证标识。 */
function splitTopLevel(expression, operator) {
  const parts = [];
  let depth = 0;
  let start = 0;
  const upper = expression.toUpperCase();
  const token = ` ${operator} `;
  for (let index = 0; index < expression.length; index += 1) {
    const char = expression[index];
    if (char === "(") depth += 1;
    else if (char === ")") depth -= 1;
    else if (depth === 0 && upper.startsWith(token, index)) {
      parts.push(expression.slice(start, index).trim());
      start = index + token.length;
      index += token.length - 1;
    }
  }
  if (start === 0) return [expression];
  parts.push(expression.slice(start).trim());
  return parts.filter(Boolean);
}

function classifyAtomic(expression) {
  const value = stripOuterParens(expression);
  if (!value) return "unresolved";

  const withParts = splitTopLevel(value, "WITH");
  if (withParts.length > 1) {
    // 例外通常只增加权限，不会把 GPL 变成宽松许可；保守地按基础许可证分类。
    return evaluate(withParts[0]);
  }

  if (/^\(?missing\)?$/i.test(value)) return "unresolved";
  if (/^UNLICENSED\b/i.test(value)) return "unresolved";
  if (/^SEE LICEN[CS]E IN\b/i.test(value)) return "unresolved";
  if (/^(UNKNOWN|N\/A)$/i.test(value)) return "unresolved";

  if (/\bAGPL/i.test(value)) return "red-agpl";
  if (/\bLGPL/i.test(value)) return "yellow-lgpl";
  if (/\bGPL/i.test(value)) return "red-gpl";
  if (/^(MPL|EPL|CDDL)/i.test(value)) return "yellow-weak";
  if (/^(BUSL|BSL|Elastic|SSPL|PolyForm|FSL|CAL-1)/i.test(value)) return "red-semiopen";
  if (/^CC-BY-NC/i.test(value)) return "red-nc";
  if (/^CC-BY-(ND|SA)/i.test(value)) return "yellow-cc";
  if (GREEN.test(value)) return "green";
  return "review";
}

function evaluate(rawExpression) {
  const expression = stripOuterParens(normalizeExpression(rawExpression));
  if (!expression) return "unresolved";
  if (!isBalanced(expression)) return "review";

  const orParts = splitTopLevel(expression, "OR");
  if (orParts.length > 1) {
    return orParts
      .map((part) => evaluate(part))
      .reduce((best, candidate) =>
        licenseSeverity(candidate) < licenseSeverity(best) ? candidate : best,
      );
  }

  const andParts = splitTopLevel(expression, "AND");
  if (andParts.length > 1) {
    return andParts
      .map((part) => evaluate(part))
      .reduce((worst, candidate) =>
        licenseSeverity(candidate) > licenseSeverity(worst) ? candidate : worst,
      );
  }

  return classifyAtomic(expression);
}

/**
 * 把一个 SPDX 风格许可证表达式分类为 `scripts/licenses.ts` 的 bucket。
 * 解析失败或空表达式按 review/unresolved 处理，绝不返回 green。
 */
export function classifyLicenseExpression(raw) {
  try {
    return evaluate(raw);
  } catch {
    return "review";
  }
}
