import { invoke } from '@tauri-apps/api/core';
import { CursorAccount } from '../types/cursor';
import { parseCursorUsageEvents, type CursorUsageEventDisplay } from '../types/cursorUsage';

export interface CursorOAuthLoginStartResponse {
  loginId: string;
  verificationUri: string;
  expiresIn: number;
  intervalSeconds: number;
}

/** Cursor OAuth: 开始登录（生成 PKCE，返回浏览器 URL） */
export async function startCursorOAuthLogin(): Promise<CursorOAuthLoginStartResponse> {
  return await invoke('cursor_oauth_login_start');
}

/** Cursor OAuth: 等待轮询完成（用户在浏览器完成登录后返回账号） */
export async function completeCursorOAuthLogin(loginId: string): Promise<CursorAccount> {
  return await invoke('cursor_oauth_login_complete', { loginId });
}

/** Cursor OAuth: 取消登录 */
export async function cancelCursorOAuthLogin(loginId?: string): Promise<void> {
  return await invoke('cursor_oauth_login_cancel', { loginId: loginId ?? null });
}

export async function listCursorAccounts(): Promise<CursorAccount[]> {
  return await invoke('list_cursor_accounts');
}

export async function deleteCursorAccount(accountId: string): Promise<void> {
  return await invoke('delete_cursor_account', { accountId });
}

export async function deleteCursorAccounts(accountIds: string[]): Promise<void> {
  return await invoke('delete_cursor_accounts', { accountIds });
}

export async function importCursorFromJson(jsonContent: string): Promise<CursorAccount[]> {
  return await invoke('import_cursor_from_json', { jsonContent });
}

export async function importCursorFromLocal(): Promise<CursorAccount[]> {
  return await invoke('import_cursor_from_local');
}

export async function exportCursorAccounts(accountIds: string[]): Promise<string> {
  return await invoke('export_cursor_accounts', { accountIds });
}

export async function refreshCursorToken(accountId: string): Promise<CursorAccount> {
  return await invoke('refresh_cursor_token', { accountId });
}

export async function refreshAllCursorTokens(): Promise<number> {
  return await invoke('refresh_all_cursor_tokens');
}

export async function addCursorAccountWithToken(accessToken: string): Promise<CursorAccount> {
  return await invoke('add_cursor_account_with_token', { accessToken });
}

export async function openCursorDashboard(accountId: string): Promise<void> {
  return await invoke('open_cursor_dashboard', { accountId });
}

export async function fetchCursorReferralStatus(accountId: string): Promise<CursorAccount> {
  return await invoke('fetch_cursor_referral_status', { accountId });
}

export async function fetchCursorAggregatedUsage(
  accountId: string,
  startDate: number,
  endDate: number,
  teamId = -1,
): Promise<unknown> {
  return await invoke('fetch_cursor_aggregated_usage', {
    accountId,
    startDate,
    endDate,
    teamId,
  });
}

export async function fetchCursorUsageEvents(
  accountId: string,
  startDate: number,
  endDate: number,
  page = 1,
  pageSize = 20,
  teamId = 0,
): Promise<unknown> {
  return await invoke('fetch_cursor_usage_events', {
    accountId,
    teamId,
    startDate: String(startDate),
    endDate: String(endDate),
    page,
    pageSize,
  });
}

export async function fetchCursorUserAnalytics(
  accountId: string,
  startDate: number,
  endDate: number,
  teamId = 0,
  userId = 0,
): Promise<unknown> {
  return await invoke('fetch_cursor_user_analytics', {
    accountId,
    teamId,
    userId,
    startDate: String(startDate),
    endDate: String(endDate),
  });
}

export async function fetchAllCursorUsageEvents(
  accountId: string,
  startDate: number,
  endDate: number,
  teamId = 0,
  pageSize = 500,
): Promise<unknown> {
  let page = 1;
  let totalCount = 0;
  const usageEventsDisplay: CursorUsageEventDisplay[] = [];

  while (true) {
    const raw = await fetchCursorUsageEvents(
      accountId,
      startDate,
      endDate,
      page,
      pageSize,
      teamId,
    );
    const parsed = parseCursorUsageEvents(raw);
    if (!parsed) break;

    totalCount = parsed.totalUsageEventsCount;
    usageEventsDisplay.push(...parsed.usageEventsDisplay);

    if (
      parsed.usageEventsDisplay.length === 0
      || usageEventsDisplay.length >= totalCount
    ) {
      break;
    }
    page += 1;
  }

  return {
    totalUsageEventsCount: totalCount,
    usageEventsDisplay,
  };
}

export async function updateCursorAccountTags(accountId: string, tags: string[]): Promise<CursorAccount> {
  return await invoke('update_cursor_account_tags', { accountId, tags });
}

export async function getCursorAccountsIndexPath(): Promise<string> {
  return await invoke('get_cursor_accounts_index_path');
}

export async function injectCursorAccount(accountId: string): Promise<string> {
  return await invoke('inject_cursor_account', { accountId });
}
