export interface CursorAccount {
  id: string;
  email: string;
  auth_id?: string | null;
  name?: string | null;
  tags?: string[] | null;

  access_token: string;
  refresh_token?: string | null;

  membership_type?: string | null;
  subscription_status?: string | null;
  sign_up_type?: string | null;

  cursor_auth_raw?: unknown;
  cursor_usage_raw?: unknown;
  cursor_credit_grants_raw?: unknown;
  cursor_free_credit_usage_raw?: unknown;
  cursor_referral_raw?: unknown;

  status?: string | null;
  status_reason?: string | null;
  quota_query_last_error?: string | null;
  quota_query_last_error_at?: number | null;

  created_at: number;
  last_used: number;

  plan_type?: string;
  quota?: CursorQuota;
}

export interface CursorQuota {
  hourly_percentage: number;
  hourly_reset_time?: number | null;
  weekly_percentage: number;
  weekly_reset_time?: number | null;
  raw_data?: unknown;
}

export type CursorPlanBadge =
  | 'FREE'
  | 'PRO'
  | 'PRO_PLUS'
  | 'ENTERPRISE'
  | 'FREE_TRIAL'
  | 'ULTRA'
  | 'UNKNOWN';

function normalizeCursorMembershipType(membershipType?: string | null): string {
  const normalized = (membershipType || '').toLowerCase().trim();
  if (!normalized) return '';
  if (normalized === 'pro_student') return 'pro';
  if (normalized === 'business' || normalized === 'team') return 'enterprise';
  return normalized;
}

function getCursorAuthRawObject(account: CursorAccount): Record<string, unknown> | null {
  if (!account.cursor_auth_raw || typeof account.cursor_auth_raw !== 'object') {
    return null;
  }
  return account.cursor_auth_raw as Record<string, unknown>;
}

function getCursorAuthRawString(
  account: CursorAccount,
  ...keys: string[]
): string | null {
  const raw = getCursorAuthRawObject(account);
  if (!raw) return null;
  for (const key of keys) {
    const value = raw[key];
    if (typeof value === 'string') {
      const trimmed = value.trim();
      if (trimmed) return trimmed;
    }
  }
  return null;
}

function parseBoolLike(value: unknown): boolean | null {
  if (typeof value === 'boolean') return value;
  if (typeof value === 'string') {
    const normalized = value.toLowerCase().trim();
    if (normalized === 'true') return true;
    if (normalized === 'false') return false;
  }
  return null;
}

function getCursorAuthRawBool(
  account: CursorAccount,
  ...keys: string[]
): boolean | null {
  const raw = getCursorAuthRawObject(account);
  if (!raw) return null;
  for (const key of keys) {
    const parsed = parseBoolLike(raw[key]);
    if (parsed !== null) return parsed;
  }
  return null;
}

function isCursorEnterpriseAccount(account: CursorAccount): boolean {
  const explicitEnterprise = getCursorAuthRawBool(
    account,
    'isEnterprise',
    'is_enterprise',
  );
  if (explicitEnterprise !== null) {
    return explicitEnterprise;
  }

  const teamMembershipType = getCursorAuthRawString(
    account,
    'teamMembershipType',
    'team_membership_type',
  );
  if (teamMembershipType) {
    const normalizedTeam = teamMembershipType.toLowerCase();
    if (normalizedTeam.includes('enterprise')) {
      return true;
    }
    if (
      normalizedTeam.includes('self_serve') ||
      normalizedTeam.includes('selfserve')
    ) {
      return false;
    }
  }

  const isTeamMember = getCursorAuthRawBool(
    account,
    'isTeamMember',
    'is_team_member',
  );
  if (isTeamMember !== null) {
    return !isTeamMember;
  }

  return false;
}

function resolveCursorPlanLabel(account: CursorAccount): string {
  const plan = getCursorPlanBadge(account);
  const subscriptionStatus = (account.subscription_status || '')
    .toLowerCase()
    .trim();
  const isTrialing = subscriptionStatus === 'trialing';

  switch (plan) {
    case 'ENTERPRISE':
      return isCursorEnterpriseAccount(account) ? 'Enterprise' : 'Team';
    case 'ULTRA':
      return 'Ultra';
    case 'PRO_PLUS':
      return isTrialing ? 'Pro+ Trial' : 'Pro+';
    case 'PRO':
      return isTrialing ? 'Pro Trial' : 'Pro';
    case 'FREE_TRIAL':
      return 'Pro Trial';
    case 'FREE':
      return 'Free';
    default:
      return 'Unknown';
  }
}

export function getCursorPlanBadge(account: CursorAccount): CursorPlanBadge {
  const membership = normalizeCursorMembershipType(account.membership_type);
  switch (membership) {
    case 'free':
      return 'FREE';
    case 'pro':
      return 'PRO';
    case 'pro_plus':
      return 'PRO_PLUS';
    case 'enterprise':
      return 'ENTERPRISE';
    case 'free_trial':
      return 'FREE_TRIAL';
    case 'ultra':
      return 'ULTRA';
    default:
      return membership ? (membership.toUpperCase() as CursorPlanBadge) : 'UNKNOWN';
  }
}

export function getCursorPlanDisplayName(account: CursorAccount): string {
  return resolveCursorPlanLabel(account);
}

export function getCursorPlanBadgeClass(
  planType?: string | null,
  account?: CursorAccount,
): string {
  const normalized = normalizeCursorMembershipType(planType);
  switch (normalized) {
    case 'ultra':
      return 'ultra';
    case 'enterprise':
      return account && !isCursorEnterpriseAccount(account) ? 'team' : 'enterprise';
    case 'pro_plus':
      return 'plus';
    case 'pro':
    case 'free_trial':
      return 'pro';
    case 'free':
      return 'free';
    default:
      return 'unknown';
  }
}

export function getCursorAccountDisplayEmail(account: CursorAccount): string {
  const email = account.email?.trim();
  if (email) return email;
  const name = account.name?.trim();
  if (name) return name;
  return account.id;
}

export type CursorUsage = {
  inlineSuggestionsUsedPercent: number | null;
  chatMessagesUsedPercent: number | null;
  allowanceResetAt?: number | null;
  planUsedCents?: number | null;
  planLimitCents?: number | null;
  totalPercentUsed?: number | null;
  autoPercentUsed?: number | null;
  apiPercentUsed?: number | null;
  onDemandUsedCents?: number | null;
  onDemandLimitCents?: number | null;
  teamOnDemandUsedCents?: number | null;
  teamOnDemandLimitCents?: number | null;
  onDemandEnabled?: boolean | null;
  onDemandLimitType?: string | null;
  isUnlimited?: boolean;
};

export type CursorOnDemandSummary = {
  isTeamLimit: boolean;
  usedCents: number;
  limitCents: number | null;
  hasFixedLimit: boolean;
  isUnlimited: boolean;
  isDisabled: boolean;
};

function getPath(obj: unknown, ...keys: string[]): unknown {
  let cur: unknown = obj;
  for (const k of keys) {
    if (cur == null || typeof cur !== 'object') return undefined;
    cur = (cur as Record<string, unknown>)[k];
  }
  return cur;
}

/** 从对象中取数字，支持多 key（camelCase + snake_case），API 可能返回任一种 */
function pickNumber(obj: unknown, ...candidateKeys: string[]): number | null {
  if (obj == null || typeof obj !== 'object') return null;
  const o = obj as Record<string, unknown>;
  for (const key of candidateKeys) {
    const val = o[key];
    if (val !== undefined && val !== null) {
      const n = typeof val === 'number' ? val : Number(val);
      if (Number.isFinite(n)) return n;
    }
  }
  return null;
}

function pickBoolean(obj: unknown, ...candidateKeys: string[]): boolean | null {
  if (obj == null || typeof obj !== 'object') return null;
  const o = obj as Record<string, unknown>;
  for (const key of candidateKeys) {
    const val = o[key];
    if (typeof val === 'boolean') return val;
    if (typeof val === 'string') {
      const normalized = val.toLowerCase().trim();
      if (normalized === 'true') return true;
      if (normalized === 'false') return false;
    }
  }
  return null;
}

function pickString(obj: unknown, ...candidateKeys: string[]): string | null {
  if (obj == null || typeof obj !== 'object') return null;
  const o = obj as Record<string, unknown>;
  for (const key of candidateKeys) {
    const val = o[key];
    if (typeof val === 'string' && val.trim()) {
      return val.trim();
    }
  }
  return null;
}

export function getCursorUsage(account: CursorAccount): CursorUsage {
  const raw = account.cursor_usage_raw;
  if (!raw || typeof raw !== 'object') {
    return { inlineSuggestionsUsedPercent: null, chatMessagesUsedPercent: null };
  }

  const plan =
    getPath(raw, 'individualUsage', 'plan') ??
    getPath(raw, 'individual_usage', 'plan') ??
    getPath(raw, 'planUsage') ??
    getPath(raw, 'plan_usage');
  const individualOnDemand =
    getPath(raw, 'individualUsage', 'onDemand') ??
    getPath(raw, 'individual_usage', 'onDemand');
  const teamOnDemand =
    getPath(raw, 'teamUsage', 'onDemand') ??
    getPath(raw, 'team_usage', 'onDemand');
  const spendLimitUsage =
    getPath(raw, 'spendLimitUsage') ??
    getPath(raw, 'spend_limit_usage');
  const onDemand = individualOnDemand ?? spendLimitUsage;

  const totalPct = pickNumber(plan, 'totalPercentUsed', 'total_percent_used');
  const autoPct = pickNumber(plan, 'autoPercentUsed', 'auto_percent_used');
  const apiPct = pickNumber(plan, 'apiPercentUsed', 'api_percent_used');
  const planUsed = pickNumber(plan, 'used', 'totalSpend', 'total_spend');
  const planLimit = pickNumber(plan, 'limit');
  const odUsed = pickNumber(
    onDemand,
    'used',
    'totalSpend',
    'total_spend',
    'individualUsed',
    'individual_used',
  );
  const odLimit = pickNumber(
    onDemand,
    'limit',
    'individualLimit',
    'individual_limit',
    'pooledLimit',
    'pooled_limit',
  );
  const teamOdUsed =
    pickNumber(teamOnDemand, 'used') ??
    pickNumber(
      spendLimitUsage,
      'pooledUsed',
      'pooled_used',
      'overallUsed',
      'overall_used',
    );
  const teamOdLimit =
    pickNumber(teamOnDemand, 'limit') ??
    pickNumber(
      spendLimitUsage,
      'pooledLimit',
      'pooled_limit',
      'overallLimit',
      'overall_limit',
    );
  const odEnabled = pickBoolean(individualOnDemand, 'enabled');
  const rawObj = raw as Record<string, unknown>;
  const isUnlimited =
    rawObj.isUnlimited === true || rawObj.is_unlimited === true;
  const limitTypeRaw =
    rawObj.limitType ??
    rawObj.limit_type ??
    (spendLimitUsage && typeof spendLimitUsage === 'object'
      ? (spendLimitUsage as Record<string, unknown>).limitType ??
        (spendLimitUsage as Record<string, unknown>).limit_type
      : undefined);
  const onDemandLimitType =
    typeof limitTypeRaw === 'string' && limitTypeRaw.trim()
      ? limitTypeRaw.trim().toLowerCase()
      : null;
  const billingEndRaw =
    rawObj.billingCycleEnd ?? rawObj.billing_cycle_end;
  let resetAt: number | null = null;
  if (typeof billingEndRaw === 'string' && billingEndRaw) {
    const ts = new Date(billingEndRaw).getTime();
    if (Number.isFinite(ts)) resetAt = Math.floor(ts / 1000);
  }

  const ratioPct =
    planUsed != null && planLimit != null && planLimit > 0
      ? (planUsed / planLimit) * 100
      : null;
  const totalBase = totalPct ?? ratioPct;
  const usedPct =
    totalBase == null
      ? null
      : totalBase > 0 && totalBase < 1
        ? 1
        : Math.min(100, Math.max(0, totalBase));

  return {
    inlineSuggestionsUsedPercent: usedPct,
    chatMessagesUsedPercent: null,
    allowanceResetAt: resetAt,
    planUsedCents: planUsed,
    planLimitCents: planLimit,
    totalPercentUsed: totalPct,
    autoPercentUsed: autoPct,
    apiPercentUsed: apiPct,
    onDemandUsedCents: odUsed,
    onDemandLimitCents: odLimit,
    teamOnDemandUsedCents: teamOdUsed,
    teamOnDemandLimitCents: teamOdLimit,
    onDemandEnabled: odEnabled,
    onDemandLimitType,
    isUnlimited,
  };
}

export function getCursorOnDemandSummary(usage: CursorUsage): CursorOnDemandSummary {
  const limitType = (usage.onDemandLimitType || '').toLowerCase();
  const isTeamLimit = limitType === 'team';
  // Team accounts must stay on one quota scope. Prefer team metrics and only
  // fall back to the normalized shared fields when the team-specific field is absent.
  const usedCents = isTeamLimit
    ? (usage.teamOnDemandUsedCents ?? usage.onDemandUsedCents ?? 0)
    : (usage.onDemandUsedCents ?? 0);
  const limitCents = isTeamLimit
    ? (usage.teamOnDemandLimitCents ?? usage.onDemandLimitCents ?? null)
    : (usage.onDemandLimitCents ?? null);
  const hasFixedLimit = limitCents != null && limitCents > 0;
  const isUnlimited = !hasFixedLimit && usage.onDemandEnabled === true && !isTeamLimit;
  const isDisabled = !hasFixedLimit && !isUnlimited;

  return {
    isTeamLimit,
    usedCents,
    limitCents,
    hasFixedLimit,
    isUnlimited,
    isDisabled,
  };
}

export function formatCursorUsageDollars(cents: number | null | undefined): string {
  if (cents == null) return '—';
  return `$${(cents / 100).toFixed(2)}`;
}

export function formatCursorCreditGrantsValue(
  usedCents: number | null | undefined,
  totalCents: number | null | undefined,
): string {
  if (usedCents != null && totalCents != null) {
    return `$${Math.round(usedCents / 100)} / $${Math.round(totalCents / 100)}`;
  }
  if (usedCents != null) {
    return `$${Math.round(usedCents / 100)}`;
  }
  if (totalCents != null) {
    return `$${Math.round(totalCents / 100)}`;
  }
  return '—';
}

export const CURSOR_PAST_DUE_FILTER = 'PAST_DUE';

export function isCursorAccountPastDue(account: CursorAccount): boolean {
  return (account.subscription_status || '').trim() === 'past_due';
}

export function isCursorAccountBanned(account: CursorAccount): boolean {
  const status = (account.status || '').toLowerCase();
  const reason = (account.status_reason || '').toLowerCase();
  return status === 'banned' || status === 'forbidden' ||
    reason.includes('banned') || reason.includes('suspended') || reason.includes('disabled');
}

export function hasCursorQuotaData(account: CursorAccount): boolean {
  return account.cursor_usage_raw != null;
}

export type CursorCreditGrants = {
  hasGrants: boolean;
  remainingCents: number | null;
  totalCents: number | null;
  usedCents: number | null;
  expiresAt: number | null;
  exhausted?: boolean;
};

export type CursorReferralStatus = {
  eligible: boolean;
  referralCode: string | null;
  referralLink: string | null;
  /** Rewards earned this billing cycle (matches Cursor "rewardedReferrals"). */
  rewardsEarnedThisCycle: number | null;
  maxRewardsPerCycle: number | null;
  /** People invited this cycle (API: invitedReferrals). */
  invitedReferrals: number | null;
  creditEarnedThisCycleCents: number | null;
  lifetimeCreditEarnedCents: number | null;
};

function buildReferralLinkFromRoot(
  root: Record<string, unknown>,
  code: string | null,
): string | null {
  const urlPath = pickString(root, 'urlPath', 'url_path');
  if (urlPath) {
    if (urlPath.startsWith('http://') || urlPath.startsWith('https://')) {
      return urlPath;
    }
    return `https://cursor.com${urlPath.startsWith('/') ? urlPath : `/${urlPath}`}`;
  }

  const existing = pickString(
    root,
    'referralLink',
    'referral_link',
    'referralUrl',
    'referral_url',
    'link',
    'url',
  );
  if (existing) {
    if (existing.startsWith('http://') || existing.startsWith('https://')) {
      return existing;
    }
    return `https://cursor.com${existing.startsWith('/') ? existing : `/${existing}`}`;
  }

  return buildReferralLink(code, null);
}

function inferMaxRewardsPerCycle(root: Record<string, unknown>): number {
  const explicit = pickNumber(
    root,
    'maxRewardsPerBillingCycle',
    'maxRewardsPerCycle',
    'maxReferralsPerCycle',
    'redemptionCapPerCycle',
    'monthlyRewardCap',
    'maxRewards',
  );
  if (explicit != null && explicit > 0) {
    return explicit;
  }

  const description = pickString(root, 'description');
  if (description) {
    const match =
      description.match(/(?:valid for|up to)\s+(\d+)\s+rewards?/i) ??
      description.match(/(\d+)\s+rewards?\s+per\s+(?:month|billing cycle)/i);
    if (match) {
      return Number(match[1]);
    }
  }

  return 10;
}

function extractReferralCode(value: Record<string, unknown>): string | null {
  const direct = pickString(
    value,
    'code',
    'referralCode',
    'referral_code',
    'p2pReferralCode',
    'p2p_referral_code',
  );
  if (direct) {
    return direct;
  }
  const link = pickString(
    value,
    'referralLink',
    'referral_link',
    'referralUrl',
    'referral_url',
    'link',
    'urlPath',
    'url_path',
  );
  if (!link) return null;
  try {
    const url = new URL(link.startsWith('http') ? link : `https://cursor.com${link.startsWith('/') ? link : `/${link}`}`);
    const code = url.searchParams.get('code');
    return code?.trim() || null;
  } catch {
    const match = link.match(/[?&]code=([^&]+)/i);
    return match?.[1]?.trim() || null;
  }
}

function buildReferralLink(code: string | null, existing: string | null): string | null {
  if (existing) return existing;
  if (!code) return null;
  return `https://cursor.com/referral?code=${encodeURIComponent(code)}`;
}

export function getCursorReferralStatus(
  account: CursorAccount,
): CursorReferralStatus | null {
  const raw = account.cursor_referral_raw;
  if (!raw || typeof raw !== 'object') return null;

  const root = raw as Record<string, unknown>;
  const eligibleFlag = pickBoolean(root, 'isEligible', 'eligible');
  const referralCode = extractReferralCode(root);
  const referralLink = buildReferralLinkFromRoot(root, referralCode);

  const rewardsEarnedThisCycle = pickNumber(
    root,
    'rewardedReferrals',
    'invitedRewardEarned',
    'rewardsEarnedThisBillingCycle',
    'rewardsEarnedThisCycle',
    'rewardsEarnedThisCycleCount',
    'numReferralsThisCycle',
    'referralsThisCycle',
    'referralCountThisCycle',
    'rewardsEarned',
  );
  const maxRewardsPerCycle = inferMaxRewardsPerCycle(root);
  const invitedReferrals = pickNumber(
    root,
    'invitedReferrals',
    'successfulReferrals',
    'invitedSignedUp',
  );
  const creditEarnedThisCycleCents = pickNumber(
    root,
    'cycleCreditCentsEarned',
    'creditEarnedThisBillingCycleCents',
    'creditEarnedThisCycleCents',
    'billingCycleCreditEarnedCents',
    'creditEarnedThisCycle',
    'cycleCreditEarnedCents',
  );
  const lifetimeCreditEarnedCents = pickNumber(
    root,
    'totalCreditCentsEarned',
    'lifetimeCreditEarnedCents',
    'totalCreditEarnedCents',
    'lifetimeCreditCents',
    'totalLifetimeCreditCents',
  );

  const eligible =
    eligibleFlag === true ||
    (eligibleFlag !== false && (referralCode != null || referralLink != null));

  if (!eligible) return null;

  return {
    eligible: true,
    referralCode,
    referralLink,
    rewardsEarnedThisCycle,
    maxRewardsPerCycle,
    invitedReferrals,
    creditEarnedThisCycleCents,
    lifetimeCreditEarnedCents,
  };
}

export function hasCursorReferralEligibility(account: CursorAccount): boolean {
  return getCursorReferralStatus(account)?.eligible === true;
}

function parseCentsValue(value: unknown): number | null {
  if (value == null) return null;
  if (typeof value === 'number' && Number.isFinite(value)) return value;
  if (typeof value === 'string') {
    const trimmed = value.trim();
    if (!trimmed) return null;
    const n = Number(trimmed);
    if (Number.isFinite(n)) return n;
  }
  return null;
}

function parseTimestampValue(value: unknown): number | null {
  if (value == null) return null;
  if (typeof value === 'number' && Number.isFinite(value)) {
    return value > 1_000_000_000_000 ? Math.floor(value / 1000) : Math.floor(value);
  }
  if (typeof value === 'string') {
    const trimmed = value.trim();
    if (!trimmed) return null;
    const asNumber = Number(trimmed);
    if (Number.isFinite(asNumber)) {
      return asNumber > 1_000_000_000_000
        ? Math.floor(asNumber / 1000)
        : Math.floor(asNumber);
    }
    const ts = new Date(trimmed).getTime();
    if (Number.isFinite(ts)) return Math.floor(ts / 1000);
  }
  return null;
}

function extractGrantExpiryTimestamps(value: unknown): number[] {
  const timestamps: number[] = [];

  const visit = (node: unknown) => {
    if (node == null) return;
    if (Array.isArray(node)) {
      node.forEach(visit);
      return;
    }
    if (typeof node !== 'object') return;

    const obj = node as Record<string, unknown>;
    for (const [key, raw] of Object.entries(obj)) {
      const normalizedKey = key.toLowerCase();
      if (
        normalizedKey.includes('expire') ||
        normalizedKey.includes('expiry') ||
        normalizedKey.includes('expiresat') ||
        normalizedKey.includes('expiration')
      ) {
        const parsed = parseTimestampValue(raw);
        if (parsed != null) timestamps.push(parsed);
      }
    }

    for (const raw of Object.values(obj)) {
      if (raw != null && typeof raw === 'object') {
        visit(raw);
      }
    }
  };

  visit(value);
  return timestamps;
}

export function getCursorCreditGrants(
  account: CursorAccount,
): CursorCreditGrants | null {
  const raw = account.cursor_credit_grants_raw;
  if (!raw || typeof raw !== 'object') return null;

  const root = raw as Record<string, unknown>;
  const balance =
    (root.balance && typeof root.balance === 'object'
      ? (root.balance as Record<string, unknown>)
      : root) ?? root;

  const hasCreditGrants = pickBoolean(
    balance,
    'hasCreditGrants',
    'has_credit_grants',
  );
  const totalCents = parseCentsValue(
    balance.totalCents ??
      balance.total_cents ??
      balance.grantTotalCents ??
      balance.grant_total_cents,
  );
  const usedCents = parseCentsValue(
    balance.usedCents ??
      balance.used_cents ??
      balance.grantUsedCents ??
      balance.grant_used_cents,
  );
  const remainingCentsRaw = parseCentsValue(
    balance.remainingCents ??
      balance.remaining_cents ??
      balance.balanceCents ??
      balance.balance_cents,
  );
  const remainingCents =
    remainingCentsRaw ??
    (totalCents != null && usedCents != null
      ? Math.max(0, totalCents - usedCents)
      : null);

  const grantsNode = root.grants ?? root.creditGrants ?? root.credit_grants;
  const expiryCandidates = extractGrantExpiryTimestamps(grantsNode);
  const nowSec = Math.floor(Date.now() / 1000);
  const futureExpiries = expiryCandidates.filter((ts) => ts >= nowSec);
  const expiresAt =
    (futureExpiries.length > 0
      ? Math.min(...futureExpiries)
      : expiryCandidates.length > 0
        ? Math.min(...expiryCandidates)
        : null) ??
    parseTimestampValue(
      balance.expiresAt ??
        balance.expires_at ??
        balance.expiryDate ??
        balance.expiry_date,
    );

  const historical =
    root.historical === true ||
    (balance as Record<string, unknown>).historical === true;
  const peak =
    root.peak && typeof root.peak === 'object'
      ? (root.peak as Record<string, unknown>)
      : null;
  const peakTotalCents = peak
    ? parseCentsValue(
        peak.totalCents ?? peak.total_cents ?? peak.grantTotalCents,
      )
    : null;
  const peakUsedCents = peak
    ? parseCentsValue(peak.usedCents ?? peak.used_cents ?? peak.grantUsedCents)
    : null;

  let resolvedTotal = totalCents ?? peakTotalCents;
  let resolvedUsed = usedCents ?? peakUsedCents;
  let resolvedRemaining = remainingCents;
  if (historical && resolvedTotal != null) {
    resolvedRemaining = 0;
    resolvedUsed = resolvedUsed ?? resolvedTotal;
  }

  const hasGrants =
    hasCreditGrants === true ||
    historical ||
    (resolvedTotal != null && resolvedTotal > 0) ||
    (resolvedRemaining != null && resolvedRemaining > 0);

  if (!hasGrants) return null;

  const exhausted =
    historical ||
    (resolvedTotal != null &&
      resolvedTotal > 0 &&
      (resolvedRemaining ?? 0) <= 0 &&
      (resolvedUsed ?? 0) >= resolvedTotal);

  return {
    hasGrants: true,
    remainingCents: resolvedRemaining,
    totalCents: resolvedTotal,
    usedCents: resolvedUsed,
    expiresAt,
    exhausted,
  };
}

export function hasCursorCreditGrants(account: CursorAccount): boolean {
  return getCursorCreditGrants(account) != null;
}

export function getCursorFreeCreditUsedCents(account: CursorAccount): number | null {
  const raw = account.cursor_free_credit_usage_raw;
  if (!raw || typeof raw !== 'object') return null;
  const root = raw as Record<string, unknown>;
  const cents = parseCentsValue(root.usedCents ?? root.used_cents);
  if (cents == null || cents <= 0) return null;
  return cents;
}

function deriveCursorGrantUsedCents(grants: CursorCreditGrants): number | null {
  return (
    grants.usedCents ??
    (grants.totalCents != null && grants.remainingCents != null
      ? Math.max(0, grants.totalCents - grants.remainingCents)
      : null)
  );
}

/** Credit-grants API returned a non-zero / usable balance snapshot. */
function hasValidCursorCreditGrantsApiData(grants: CursorCreditGrants): boolean {
  if (grants.remainingCents != null && grants.remainingCents > 0) return true;
  if (grants.totalCents != null && grants.totalCents > 0) return true;
  if (grants.usedCents != null && grants.usedCents > 0) return true;
  return false;
}

export interface CursorCreditGrantsQuotaDisplay {
  usedCents: number;
  totalCents: number;
  valueText: string;
  percentage: number;
}

export function resolveCursorCreditGrantsQuotaDisplay(
  account: CursorAccount,
): CursorCreditGrantsQuotaDisplay | null {
  const grants = getCursorCreditGrants(account);
  const freeCreditUsed = getCursorFreeCreditUsedCents(account);

  // 1. Credit-grants API has valid metrics → show API data only (ignore FREE_CREDIT events).
  if (grants && hasValidCursorCreditGrantsApiData(grants)) {
    const total = grants.totalCents;
    const used = deriveCursorGrantUsedCents(grants) ?? 0;
    const effectiveTotal = total ?? used;
    const percentage =
      total != null && total > 0
        ? Math.min(100, Math.max(0, (used / total) * 100))
        : 0;

    return {
      usedCents: used,
      totalCents: effectiveTotal,
      valueText: formatCursorCreditGrantsValue(used, total),
      percentage,
    };
  }

  // 2. API empty / all-zero → fall back to FREE_CREDIT event aggregate.
  if (freeCreditUsed != null) {
    return {
      usedCents: freeCreditUsed,
      totalCents: freeCreditUsed,
      valueText: formatCursorCreditGrantsValue(freeCreditUsed, freeCreditUsed),
      percentage: 100,
    };
  }

  // 3. Neither source has data.
  return null;
}

export function shouldShowCursorCreditGrantsQuota(account: CursorAccount): boolean {
  return resolveCursorCreditGrantsQuotaDisplay(account) != null;
}
