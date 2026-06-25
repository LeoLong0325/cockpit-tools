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
  'api-relay',
];

export function isEnabledPage(page: Page): boolean {
  return ENABLED_PAGES.includes(page);
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
