export interface CursorModelUsage {
  model_intent: string;
  input_tokens: string;
  output_tokens: string;
  cache_write_tokens: string;
  cache_read_tokens: string;
  total_cents: number;
}

export interface CursorAggregatedUsageData {
  aggregations: CursorModelUsage[];
  total_input_tokens: string;
  total_output_tokens: string;
  total_cache_write_tokens: string;
  total_cache_read_tokens: string;
  total_cost_cents: number;
}

export interface CursorUsageEventDisplay {
  timestamp: string;
  model: string;
  kind: string;
  usageBasedCosts: string;
  tokenUsage?: {
    inputTokens?: number;
    outputTokens?: number;
    cacheWriteTokens?: number;
    cacheReadTokens?: number;
    totalCents?: number;
  };
}

export interface CursorFilteredUsageEventsData {
  totalUsageEventsCount: number;
  usageEventsDisplay: CursorUsageEventDisplay[];
}

export interface CursorUserAnalyticsData {
  dailyMetrics: Array<Record<string, unknown>>;
  period: { startDate: string; endDate: string };
  totalMembersInTeam: number;
}

export type CursorUsagePeriod =
  | 'billingCycle'
  | '7days'
  | '30days'
  | 'thisMonth'
  | 'custom';

/** Cursor 配额自动刷新预设（含 1 分钟） */
export const CURSOR_QUOTA_REFRESH_PRESET_VALUES = ['-1', '1', '2', '5', '10', '15'] as const;

export const CURSOR_USAGE_EVENT_KIND_FREE_CREDIT = 'USAGE_EVENT_KIND_FREE_CREDIT';

/** Auto-routing / Composer agent models — not gifted-credit API consumption. */
export function isCursorGiftCreditUsageModel(model: string): boolean {
  const normalized = model.trim().toLowerCase();
  if (!normalized || normalized === '—') return false;
  if (normalized === 'default') return false;
  if (normalized.startsWith('composer')) return false;
  return true;
}

export interface CursorFreeCreditModelUsage {
  model: string;
  input_tokens: string;
  output_tokens: string;
  cache_write_tokens: string;
  cache_read_tokens: string;
  total_cents: number;
  event_count: number;
}

export interface CursorFreeCreditUsageSummary {
  total_input_tokens: string;
  total_output_tokens: string;
  total_cache_write_tokens: string;
  total_cache_read_tokens: string;
  total_cost_cents: number;
  event_count: number;
  models: CursorFreeCreditModelUsage[];
}

function pickString(obj: Record<string, unknown>, ...keys: string[]): string {
  for (const key of keys) {
    const value = obj[key];
    if (typeof value === 'string') return value;
  }
  return '';
}

function pickNumber(obj: Record<string, unknown>, ...keys: string[]): number {
  for (const key of keys) {
    const value = obj[key];
    if (typeof value === 'number' && Number.isFinite(value)) return value;
    if (typeof value === 'string' && value.trim()) {
      const parsed = Number(value);
      if (Number.isFinite(parsed)) return parsed;
    }
  }
  return 0;
}

export function parseCursorAggregatedUsage(raw: unknown): CursorAggregatedUsageData | null {
  if (!raw || typeof raw !== 'object') return null;
  const root = raw as Record<string, unknown>;
  const aggregationsRaw = Array.isArray(root.aggregations) ? root.aggregations : [];
  const aggregations: CursorModelUsage[] = aggregationsRaw
    .map((item) => {
      if (!item || typeof item !== 'object') return null;
      const row = item as Record<string, unknown>;
      const modelIntent = pickString(row, 'modelIntent', 'model_intent');
      if (!modelIntent) return null;
      return {
        model_intent: modelIntent,
        input_tokens: pickString(row, 'inputTokens', 'input_tokens') || '0',
        output_tokens: pickString(row, 'outputTokens', 'output_tokens') || '0',
        cache_write_tokens: pickString(row, 'cacheWriteTokens', 'cache_write_tokens') || '0',
        cache_read_tokens: pickString(row, 'cacheReadTokens', 'cache_read_tokens') || '0',
        total_cents: pickNumber(row, 'totalCents', 'total_cents'),
      };
    })
    .filter((item): item is CursorModelUsage => item != null);

  return {
    aggregations,
    total_input_tokens: pickString(root, 'totalInputTokens', 'total_input_tokens') || '0',
    total_output_tokens: pickString(root, 'totalOutputTokens', 'total_output_tokens') || '0',
    total_cache_write_tokens:
      pickString(root, 'totalCacheWriteTokens', 'total_cache_write_tokens') || '0',
    total_cache_read_tokens:
      pickString(root, 'totalCacheReadTokens', 'total_cache_read_tokens') || '0',
    total_cost_cents: pickNumber(root, 'totalCostCents', 'total_cost_cents'),
  };
}

export function parseCursorUsageEvents(raw: unknown): CursorFilteredUsageEventsData | null {
  if (!raw || typeof raw !== 'object') return null;
  const root = raw as Record<string, unknown>;
  const eventsRaw = Array.isArray(root.usageEventsDisplay)
    ? root.usageEventsDisplay
    : Array.isArray(root.usage_events_display)
      ? root.usage_events_display
      : [];
  const usageEventsDisplay: CursorUsageEventDisplay[] = eventsRaw.flatMap((item) => {
    if (!item || typeof item !== 'object') return [];
    const row = item as Record<string, unknown>;
    const tokenRaw =
      row.tokenUsage && typeof row.tokenUsage === 'object'
        ? (row.tokenUsage as Record<string, unknown>)
        : row.token_usage && typeof row.token_usage === 'object'
          ? (row.token_usage as Record<string, unknown>)
          : null;
    return [{
      timestamp: pickString(row, 'timestamp'),
      model: pickString(row, 'model'),
      kind: pickString(row, 'kind'),
      usageBasedCosts: pickString(row, 'usageBasedCosts', 'usage_based_costs'),
      tokenUsage: tokenRaw
        ? {
            inputTokens: pickNumber(tokenRaw, 'inputTokens', 'input_tokens') || undefined,
            outputTokens: pickNumber(tokenRaw, 'outputTokens', 'output_tokens') || undefined,
            cacheWriteTokens:
              pickNumber(tokenRaw, 'cacheWriteTokens', 'cache_write_tokens') || undefined,
            cacheReadTokens:
              pickNumber(tokenRaw, 'cacheReadTokens', 'cache_read_tokens') || undefined,
            totalCents: pickNumber(tokenRaw, 'totalCents', 'total_cents') || undefined,
          }
        : undefined,
    }];
  });

  return {
    totalUsageEventsCount: pickNumber(
      root,
      'totalUsageEventsCount',
      'total_usage_events_count',
    ),
    usageEventsDisplay,
  };
}

function parseEventTimestampMs(timestamp: string): number {
  const ms = Number(timestamp);
  if (Number.isFinite(ms) && ms > 0) {
    return ms;
  }
  const parsed = Date.parse(timestamp);
  return Number.isFinite(parsed) ? parsed : 0;
}

/** Cursor dashboard API 返回升序/降序混排时，展示前统一按时间倒序。 */
export function sortCursorUsageEventsDescending(
  events: CursorUsageEventDisplay[],
): CursorUsageEventDisplay[] {
  return [...events].sort(
    (left, right) => parseEventTimestampMs(right.timestamp) - parseEventTimestampMs(left.timestamp),
  );
}

/** Cursor dashboard API 分页按时间降序返回：第 1 页是最新记录。 */
export function resolveCursorUsageEventsMaxPage(totalCount: number, pageSize: number): number {
  if (!Number.isFinite(totalCount) || totalCount <= 0 || pageSize <= 0) {
    return 1;
  }
  return Math.max(1, Math.ceil(totalCount / pageSize));
}

function parseTokenCount(value: string | number | null | undefined): number {
  if (value == null || value === '') return 0;
  const num = typeof value === 'number' ? value : Number(String(value).replace(/,/g, ''));
  return Number.isFinite(num) ? num : 0;
}

export function hasCursorModelUsage(model: CursorModelUsage): boolean {
  if (model.total_cents > 0) return true;
  return [
    model.input_tokens,
    model.output_tokens,
    model.cache_write_tokens,
    model.cache_read_tokens,
  ].some((value) => parseTokenCount(value) > 0);
}

function parseUsageCostCents(value: string | null | undefined): number {
  if (!value) return 0;
  const cleaned = value.trim().replace(/,/g, '').replace(/^\$/, '');
  if (!cleaned) return 0;
  const parsed = Number(cleaned);
  if (!Number.isFinite(parsed)) return 0;
  return Math.round(parsed * 100);
}

export function parseFreeCreditEventCents(event: CursorUsageEventDisplay): number {
  let cents = event.tokenUsage?.totalCents ?? 0;
  if (cents <= 0) {
    cents = parseUsageCostCents(event.usageBasedCosts);
  }
  return cents > 0 ? Math.round(cents) : 0;
}

export function buildFreeCreditUsageSummary(
  events: CursorUsageEventDisplay[],
): CursorFreeCreditUsageSummary | null {
  const freeCreditEvents = events.filter(
    (event) =>
      event.kind === CURSOR_USAGE_EVENT_KIND_FREE_CREDIT
      && isCursorGiftCreditUsageModel(event.model),
  );
  if (freeCreditEvents.length === 0) return null;

  let inputTokens = 0;
  let outputTokens = 0;
  let cacheWriteTokens = 0;
  let cacheReadTokens = 0;
  let totalCents = 0;
  let chargedEventCount = 0;
  const modelMap = new Map<string, CursorFreeCreditModelUsage>();

  freeCreditEvents.forEach((event) => {
    const eventCents = parseFreeCreditEventCents(event);
    if (eventCents <= 0) return;

    chargedEventCount += 1;
    totalCents += eventCents;

    const tokenUsage = event.tokenUsage;
    const modelName = event.model.trim() || '—';
    const existing = modelMap.get(modelName) ?? {
      model: modelName,
      input_tokens: '0',
      output_tokens: '0',
      cache_write_tokens: '0',
      cache_read_tokens: '0',
      total_cents: 0,
      event_count: 0,
    };

    if (tokenUsage) {
      const nextInput = parseTokenCount(existing.input_tokens) + (tokenUsage.inputTokens ?? 0);
      const nextOutput = parseTokenCount(existing.output_tokens) + (tokenUsage.outputTokens ?? 0);
      const nextCacheWrite =
        parseTokenCount(existing.cache_write_tokens) + (tokenUsage.cacheWriteTokens ?? 0);
      const nextCacheRead =
        parseTokenCount(existing.cache_read_tokens) + (tokenUsage.cacheReadTokens ?? 0);
      existing.input_tokens = String(nextInput);
      existing.output_tokens = String(nextOutput);
      existing.cache_write_tokens = String(nextCacheWrite);
      existing.cache_read_tokens = String(nextCacheRead);
      inputTokens += tokenUsage.inputTokens ?? 0;
      outputTokens += tokenUsage.outputTokens ?? 0;
      cacheWriteTokens += tokenUsage.cacheWriteTokens ?? 0;
      cacheReadTokens += tokenUsage.cacheReadTokens ?? 0;
    }

    existing.total_cents += eventCents;
    existing.event_count += 1;
    modelMap.set(modelName, existing);
  });

  if (totalCents <= 0) return null;

  const models = Array.from(modelMap.values()).sort(
    (left, right) => right.total_cents - left.total_cents,
  );

  return {
    total_input_tokens: String(inputTokens),
    total_output_tokens: String(outputTokens),
    total_cache_write_tokens: String(cacheWriteTokens),
    total_cache_read_tokens: String(cacheReadTokens),
    total_cost_cents: totalCents,
    event_count: chargedEventCount,
    models,
  };
}

export function aggregateFreeCreditUsage(
  events: CursorUsageEventDisplay[],
): CursorModelUsage | null {
  const summary = buildFreeCreditUsageSummary(events);
  if (!summary) return null;

  return {
    model_intent: CURSOR_USAGE_EVENT_KIND_FREE_CREDIT,
    input_tokens: summary.total_input_tokens,
    output_tokens: summary.total_output_tokens,
    cache_write_tokens: summary.total_cache_write_tokens,
    cache_read_tokens: summary.total_cache_read_tokens,
    total_cents: summary.total_cost_cents,
  };
}

export function resolveFreeCreditUsage(
  aggregations: CursorModelUsage[],
  events: CursorUsageEventDisplay[],
): CursorModelUsage | null {
  const fromAggregated = aggregations.find(
    (item) => item.model_intent === CURSOR_USAGE_EVENT_KIND_FREE_CREDIT,
  );
  if (fromAggregated && hasCursorModelUsage(fromAggregated)) {
    return fromAggregated;
  }

  // Aggregated API row missing or zero → sum USAGE_EVENT_KIND_FREE_CREDIT events.
  const fromEvents = aggregateFreeCreditUsage(events);
  if (fromEvents && hasCursorModelUsage(fromEvents)) {
    return fromEvents;
  }

  return null;
}

export function buildUsageModelBreakdown(
  aggregations: CursorModelUsage[],
  freeCredit: CursorModelUsage | null,
): CursorModelUsage[] {
  const regular = aggregations.filter(
    (item) => item.model_intent !== CURSOR_USAGE_EVENT_KIND_FREE_CREDIT,
  );
  return freeCredit ? [...regular, freeCredit] : regular;
}

export function isCursorFreeCreditModelIntent(modelIntent: string): boolean {
  return modelIntent === CURSOR_USAGE_EVENT_KIND_FREE_CREDIT;
}

export function parseCursorUserAnalytics(raw: unknown): CursorUserAnalyticsData | null {
  if (!raw || typeof raw !== 'object') return null;
  const root = raw as Record<string, unknown>;
  const periodRaw =
    root.period && typeof root.period === 'object'
      ? (root.period as Record<string, unknown>)
      : {};
  return {
    dailyMetrics: Array.isArray(root.dailyMetrics)
      ? (root.dailyMetrics as Array<Record<string, unknown>>)
      : Array.isArray(root.daily_metrics)
        ? (root.daily_metrics as Array<Record<string, unknown>>)
        : [],
    period: {
      startDate: pickString(periodRaw, 'startDate', 'start_date'),
      endDate: pickString(periodRaw, 'endDate', 'end_date'),
    },
    totalMembersInTeam: pickNumber(
      root,
      'totalMembersInTeam',
      'total_members_in_team',
    ),
  };
}

export function getCursorUsageEventKindLabel(kind: string): string {
  const map: Record<string, string> = {
    USAGE_EVENT_KIND_INCLUDED_IN_PRO: '包含在订阅中',
    USAGE_EVENT_KIND_ERRORED_NOT_CHARGED: '错误未计费',
    USAGE_EVENT_KIND_PAID: '付费使用',
    USAGE_EVENT_KIND_FREE: '免费使用',
    USAGE_EVENT_KIND_FREE_CREDIT: '赠送额度',
  };
  return map[kind] || kind;
}

export function getCursorUsageDateRange(
  period: CursorUsagePeriod,
  customStart?: string,
  customEnd?: string,
  billingCycle?: { startMs: number; endMs: number } | null,
) {
  const now = Date.now();
  if (period === 'billingCycle') {
    if (
      billingCycle &&
      Number.isFinite(billingCycle.startMs) &&
      Number.isFinite(billingCycle.endMs)
    ) {
      return {
        startMs: Math.min(billingCycle.startMs, billingCycle.endMs),
        endMs: Math.max(billingCycle.startMs, billingCycle.endMs),
      };
    }
    return { startMs: now - 30 * 24 * 60 * 60 * 1000, endMs: now };
  }
  if (period === '7days') {
    return { startMs: now - 7 * 24 * 60 * 60 * 1000, endMs: now };
  }
  if (period === 'thisMonth') {
    const start = new Date();
    start.setDate(1);
    start.setHours(0, 0, 0, 0);
    return { startMs: start.getTime(), endMs: now };
  }
  if (period === 'custom' && customStart && customEnd) {
    const startMs = new Date(`${customStart}T00:00:00`).getTime();
    const endMs = new Date(`${customEnd}T23:59:59`).getTime();
    return { startMs, endMs };
  }
  return { startMs: now - 30 * 24 * 60 * 60 * 1000, endMs: now };
}

/** 拉取用量时使用当前时刻作为结束时间，避免缓存的 endMs 漏掉最新记录。 */
export function getCursorUsageLiveEndMs(startMs: number): number {
  return Math.max(startMs + 1, Date.now());
}

export function getCursorUsageFetchRange(startMs: number, endMs?: number) {
  const oneDayMs = 24 * 60 * 60 * 1000;
  if (typeof endMs === 'number' && Number.isFinite(endMs) && endMs < Date.now() - oneDayMs) {
    return { startMs, endMs: Math.max(startMs + 1, endMs) };
  }
  return { startMs, endMs: getCursorUsageLiveEndMs(startMs) };
}
