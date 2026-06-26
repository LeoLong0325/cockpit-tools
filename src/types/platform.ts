import { Page } from './navigation';

/** Full union kept for legacy persisted data / transfer payloads. */
export type PlatformId =
  | 'antigravity'
  | 'antigravity_ide'
  | 'codex'
  | 'claude_manager'
  | 'zed'
  | 'github-copilot'
  | 'windsurf'
  | 'kiro'
  | 'cursor'
  | 'gemini'
  | 'codebuddy'
  | 'codebuddy_cn'
  | 'qoder'
  | 'trae'
  | 'workbuddy';

/** Lite build: only Cursor, Codex, and Antigravity are shipped in the UI. */
export const ENABLED_PLATFORM_IDS = [
  'antigravity',
  'antigravity_ide',
  'codex',
  'cursor',
] as const satisfies readonly PlatformId[];

export type EnabledPlatformId = (typeof ENABLED_PLATFORM_IDS)[number];

export const ALL_PLATFORM_IDS: PlatformId[] = [...ENABLED_PLATFORM_IDS];

export const MENU_HIDDEN_PLATFORM_IDS: PlatformId[] = [];

export const MENU_VISIBLE_PLATFORM_IDS: PlatformId[] = ALL_PLATFORM_IDS.filter(
  (platformId) => !MENU_HIDDEN_PLATFORM_IDS.includes(platformId),
);

export function isEnabledPlatform(platformId: PlatformId): platformId is EnabledPlatformId {
  return (ENABLED_PLATFORM_IDS as readonly PlatformId[]).includes(platformId);
}

export function isMenuVisiblePlatform(platformId: PlatformId): boolean {
  return isEnabledPlatform(platformId) && !MENU_HIDDEN_PLATFORM_IDS.includes(platformId);
}

export const ENABLED_PAGES: Page[] = [
  'dashboard',
  'overview',
  'codex',
  'codex-api-service',
  'codex-instances',
  'cursor',
  'settings',
  'manual',
  'instances',
  'wakeup',
  'verification',
  '2fa',
];

export function isEnabledPage(page: Page): boolean {
  return ENABLED_PAGES.includes(page);
}

export function isApiRelayEnabled(): boolean {
  return isEnabledPage('api-relay');
}

export const PLATFORM_PAGE_MAP: Record<PlatformId, Page> = {
  antigravity: 'overview',
  antigravity_ide: 'overview',
  codex: 'codex',
  claude_manager: 'claude',
  zed: 'zed',
  'github-copilot': 'github-copilot',
  windsurf: 'windsurf',
  kiro: 'kiro',
  cursor: 'cursor',
  gemini: 'gemini',
  codebuddy: 'codebuddy',
  codebuddy_cn: 'codebuddy-cn',
  qoder: 'qoder',
  trae: 'trae',
  workbuddy: 'workbuddy',
};

/** Keys used by useAutoRefresh / currentAccountRefresh (not identical to PlatformId). */
export type AutoRefreshPlatformKey =
  | 'antigravity'
  | 'codex'
  | 'claude'
  | 'ghcp'
  | 'windsurf'
  | 'kiro'
  | 'cursor'
  | 'gemini'
  | 'codebuddy'
  | 'codebuddy_cn'
  | 'workbuddy'
  | 'qoder'
  | 'trae'
  | 'zed';

export const AUTO_REFRESH_KEY_TO_PLATFORM_ID: Record<AutoRefreshPlatformKey, PlatformId> = {
  antigravity: 'antigravity',
  codex: 'codex',
  claude: 'claude_manager',
  ghcp: 'github-copilot',
  windsurf: 'windsurf',
  kiro: 'kiro',
  cursor: 'cursor',
  gemini: 'gemini',
  codebuddy: 'codebuddy',
  codebuddy_cn: 'codebuddy_cn',
  workbuddy: 'workbuddy',
  qoder: 'qoder',
  trae: 'trae',
  zed: 'zed',
};

export function isAutoRefreshPlatformEnabled(key: AutoRefreshPlatformKey): boolean {
  return isEnabledPlatform(AUTO_REFRESH_KEY_TO_PLATFORM_ID[key]);
}

/** Tray menu quota refresh Tauri commands → platform id. */
export const TRAY_REFRESH_COMMAND_PLATFORM: Record<string, PlatformId> = {
  refresh_current_quota: 'antigravity',
  refresh_current_codex_quota: 'codex',
  refresh_all_claude_quotas: 'claude_manager',
  refresh_all_github_copilot_tokens: 'github-copilot',
  refresh_all_windsurf_tokens: 'windsurf',
  refresh_all_kiro_tokens: 'kiro',
  refresh_all_cursor_tokens: 'cursor',
  refresh_all_gemini_tokens: 'gemini',
  refresh_all_codebuddy_tokens: 'codebuddy',
  refresh_all_codebuddy_cn_tokens: 'codebuddy_cn',
  refresh_all_qoder_tokens: 'qoder',
  refresh_all_trae_tokens: 'trae',
  refresh_all_zed_tokens: 'zed',
};

export function isTrayRefreshCommandEnabled(command: string): boolean {
  const platformId = TRAY_REFRESH_COMMAND_PLATFORM[command];
  if (!platformId) {
    return true;
  }
  return isEnabledPlatform(platformId);
}

/** Platforms that may load accounts, auto-refresh, or token keep-alive. */
export const RESOURCE_ENABLED_PLATFORM_IDS = ENABLED_PLATFORM_IDS.filter(
  (platformId) => platformId !== 'antigravity_ide',
);

export function isResourceEnabledPlatform(platformId: PlatformId): boolean {
  if (platformId === 'antigravity_ide') {
    return isEnabledPlatform('antigravity');
  }
  return isEnabledPlatform(platformId);
}
