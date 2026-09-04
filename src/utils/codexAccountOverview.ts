import type { CodexAccount } from "../types/codex";
import {
  getCodexPlanFilterKey,
  isCodexApiKeyAccount,
  isCodexNewApiAccount,
  parseCodexSubscriptionDate,
} from "../types/codex";

export const CODEX_ZERO_QUOTA_FILTER_VALUE = "ZERO_QUOTA";
export const CODEX_EXPIRED_FILTER_VALUE = "EXPIRED";

/** OAuth 账号主窗口和周窗口剩余额度都为 0。 */
export function isCodexOverviewAccountZeroQuota(account: CodexAccount): boolean {
  if (isCodexNewApiAccount(account) || isCodexApiKeyAccount(account)) {
    return false;
  }
  const hourly = account.quota?.hourly_percentage;
  const weekly = account.quota?.weekly_percentage;
  const hasHourly = typeof hourly === "number" && Number.isFinite(hourly);
  const hasWeekly = typeof weekly === "number" && Number.isFinite(weekly);
  if (!hasHourly && !hasWeekly) return false;
  if (hasHourly && hourly! > 0) return false;
  if (hasWeekly && weekly! > 0) return false;
  return true;
}

export function isCodexOverviewAccountSubscriptionExpired(
  account: CodexAccount,
): boolean {
  if (isCodexNewApiAccount(account) || isCodexApiKeyAccount(account)) {
    return false;
  }
  const expiry = parseCodexSubscriptionDate(account.subscription_active_until);
  if (!expiry) return false;
  return expiry.getTime() <= Date.now();
}

export function matchesCodexOverviewSpecialFilter(
  account: CodexAccount,
  selectedTypes: Set<string>,
  isAbnormalAccount: (account: CodexAccount) => boolean,
): boolean {
  if (selectedTypes.has("ERROR") && isAbnormalAccount(account)) {
    return true;
  }
  if (
    selectedTypes.has(CODEX_ZERO_QUOTA_FILTER_VALUE) &&
    isCodexOverviewAccountZeroQuota(account)
  ) {
    return true;
  }
  if (
    selectedTypes.has(CODEX_EXPIRED_FILTER_VALUE) &&
    isCodexOverviewAccountSubscriptionExpired(account)
  ) {
    return true;
  }
  return selectedTypes.has(getCodexPlanFilterKey(account));
}
