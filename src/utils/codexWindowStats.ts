export const CODEX_WINDOW_FALLBACK_MINUTES = {
  primary: 5 * 60,
  secondary: 7 * 24 * 60,
} as const;

export interface CodexWindowStats {
  requestCount: number;
  inputTokens: number;
  cachedInputTokens: number;
  outputTokens: number;
  totalTokens: number;
  estimatedCostUsd: number;
  userCostUsd?: number | null;
}

/** 与 Sub2API formatCompactNumber 一致：K/M/B，1 位小数。 */
export function formatCodexCompactNumber(
  value: number | null | undefined,
  options?: { allowBillions?: boolean },
): string {
  if (value == null || !Number.isFinite(value)) return "0";
  const abs = Math.abs(value);
  const allowBillions = options?.allowBillions !== false;
  if (allowBillions && abs >= 1_000_000_000) {
    return `${(value / 1_000_000_000).toFixed(1)}B`;
  }
  if (abs >= 1_000_000) return `${(value / 1_000_000).toFixed(1)}M`;
  if (abs >= 1_000) return `${(value / 1_000).toFixed(1)}K`;
  return String(Math.trunc(value));
}

export function formatCodexWindowTokenCount(value: number): string {
  return formatCodexCompactNumber(value);
}

export function formatCodexWindowRequestCount(value: number): string {
  return formatCodexCompactNumber(value, { allowBillions: false });
}

export function formatCodexWindowCostAmount(value: number): string {
  if (!Number.isFinite(value) || value <= 0) return "0.00";
  return value.toFixed(2);
}

export function hasVisibleCodexWindowStats(
  stats?: CodexWindowStats | null,
): boolean {
  if (!stats) return false;
  return stats.requestCount > 0 || stats.totalTokens > 0;
}
