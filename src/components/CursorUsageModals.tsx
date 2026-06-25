import { useCallback, useEffect, useMemo, useState } from 'react';
import { ClipboardList, X } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { useEscClose } from '../hooks/useEscClose';
import * as cursorService from '../services/cursorService';
import { formatCursorUsageDollars } from '../types/cursor';
import {
  getCursorUsageDateRange,
  getCursorUsageEventKindLabel,
  parseCursorAggregatedUsage,
  parseCursorUsageEvents,
  parseCursorUserAnalytics,
  type CursorAggregatedUsageData,
  type CursorFilteredUsageEventsData,
  type CursorUsagePeriod,
  type CursorUserAnalyticsData,
} from '../types/cursorUsage';

function formatTokenCount(value: string | number | null | undefined): string {
  if (value == null || value === '') return '0';
  const num = typeof value === 'number' ? value : Number(String(value).replace(/,/g, ''));
  if (!Number.isFinite(num)) return String(value);
  return num.toLocaleString();
}

function formatEventTimestamp(timestamp: string): string {
  if (!timestamp) return '—';
  const ms = Number(timestamp);
  const date = Number.isFinite(ms) ? new Date(ms) : new Date(timestamp);
  if (Number.isNaN(date.getTime())) return '—';
  return date.toLocaleString();
}

interface CursorUsageDetailsModalProps {
  isOpen: boolean;
  accountId: string;
  startMs: number;
  endMs: number;
  onClose: () => void;
}

export function CursorUsageDetailsModal(props: CursorUsageDetailsModalProps) {
  const { isOpen, accountId, startMs, endMs, onClose } = props;
  const { t } = useTranslation();
  const [activeTab, setActiveTab] = useState<'events' | 'analytics'>('events');
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [eventsData, setEventsData] = useState<CursorFilteredUsageEventsData | null>(null);
  const [analyticsData, setAnalyticsData] = useState<CursorUserAnalyticsData | null>(null);
  const [currentPage, setCurrentPage] = useState(1);
  const pageSize = 20;
  useEscClose(isOpen, onClose);

  const loadData = useCallback(async () => {
    if (!isOpen || !accountId) return;
    setLoading(true);
    setError(null);
    try {
      if (activeTab === 'events') {
        const raw = await cursorService.fetchCursorUsageEvents(
          accountId,
          startMs,
          endMs,
          currentPage,
          pageSize,
        );
        setEventsData(parseCursorUsageEvents(raw));
      } else {
        const raw = await cursorService.fetchCursorUserAnalytics(accountId, startMs, endMs);
        setAnalyticsData(parseCursorUserAnalytics(raw));
      }
    } catch (err) {
      setError(String(err));
    } finally {
      setLoading(false);
    }
  }, [accountId, activeTab, currentPage, endMs, isOpen, startMs]);

  useEffect(() => {
    if (!isOpen) return;
    void loadData();
  }, [isOpen, loadData]);

  useEffect(() => {
    if (!isOpen) {
      setActiveTab('events');
      setCurrentPage(1);
      setEventsData(null);
      setAnalyticsData(null);
      setError(null);
    }
  }, [isOpen]);

  if (!isOpen) return null;

  const maxPage = eventsData
    ? Math.max(1, Math.ceil(eventsData.totalUsageEventsCount / pageSize))
    : 1;

  return (
    <div className="modal-overlay cursor-usage-details-overlay" onClick={onClose}>
      <div className="modal cursor-usage-details-modal" onClick={(event) => event.stopPropagation()}>
        <div className="modal-header">
          <h2>{t('cursor.usage.detailsTitle', '使用详情 (最近30天)')}</h2>
          <button className="modal-close" onClick={onClose} aria-label={t('common.close', '关闭')}>
            <X />
          </button>
        </div>

        <div className="modal-body cursor-usage-details-body">
          <div className="cursor-usage-tabs">
            <button
              type="button"
              className={`cursor-usage-tab ${activeTab === 'events' ? 'active' : ''}`}
              onClick={() => {
                setActiveTab('events');
                setCurrentPage(1);
              }}
            >
              {t('cursor.usage.eventsTab', '使用事件明细')}
            </button>
            <button
              type="button"
              className={`cursor-usage-tab ${activeTab === 'analytics' ? 'active' : ''}`}
              onClick={() => setActiveTab('analytics')}
            >
              {t('cursor.usage.analyticsTab', '用户分析数据')}
            </button>
          </div>

          {loading ? (
            <p className="modal-muted">{t('common.loading', '加载中...')}</p>
          ) : error ? (
            <p className="modal-error-text">{error}</p>
          ) : activeTab === 'events' && eventsData ? (
            <>
              <div className="cursor-usage-events-table-wrap">
                <table className="cursor-usage-events-table">
                  <thead>
                    <tr>
                      <th>{t('cursor.usage.colTime', '时间')}</th>
                      <th>{t('cursor.usage.colModel', '模型')}</th>
                      <th>{t('cursor.usage.colKind', '类型')}</th>
                      <th>{t('cursor.usage.colTokens', 'Token用量')}</th>
                      <th>{t('cursor.usage.colCost', '费用')}</th>
                    </tr>
                  </thead>
                  <tbody>
                    {eventsData.usageEventsDisplay.map((event, index) => (
                      <tr key={`${event.timestamp}-${index}`}>
                        <td>{formatEventTimestamp(event.timestamp)}</td>
                        <td><span className="cursor-usage-model-pill">{event.model || '—'}</span></td>
                        <td>
                          <span className={`cursor-usage-kind-pill ${event.kind.includes('ERRORED') ? 'error' : event.kind.includes('INCLUDED') ? 'included' : 'default'}`}>
                            {getCursorUsageEventKindLabel(event.kind)}
                          </span>
                        </td>
                        <td>
                          {event.tokenUsage ? (
                            <div className="cursor-usage-token-cell">
                              <div>{t('cursor.usage.inputTokens', '输入')}: {formatTokenCount(event.tokenUsage.inputTokens)}</div>
                              <div>{t('cursor.usage.outputTokens', '输出')}: {formatTokenCount(event.tokenUsage.outputTokens)}</div>
                              <div>{t('cursor.usage.cacheTokens', '缓存')}: {formatTokenCount(event.tokenUsage.cacheReadTokens)}</div>
                            </div>
                          ) : '—'}
                        </td>
                        <td>
                          {event.tokenUsage?.totalCents != null
                            ? formatCursorUsageDollars(event.tokenUsage.totalCents)
                            : event.usageBasedCosts || '—'}
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
              <div className="cursor-usage-pagination">
                <span>
                  {t('cursor.usage.pageInfo', '第 {{page}} / {{total}} 页，共 {{count}} 条', {
                    page: currentPage,
                    total: maxPage,
                    count: eventsData.totalUsageEventsCount,
                  })}
                </span>
                <div className="cursor-usage-pagination-actions">
                  <button
                    type="button"
                    className="btn btn-secondary btn-sm"
                    disabled={currentPage <= 1}
                    onClick={() => setCurrentPage((page) => Math.max(1, page - 1))}
                  >
                    {t('cursor.usage.prevPage', '上一页')}
                  </button>
                  <button
                    type="button"
                    className="btn btn-secondary btn-sm"
                    disabled={currentPage >= maxPage}
                    onClick={() => setCurrentPage((page) => page + 1)}
                  >
                    {t('cursor.usage.nextPage', '下一页')}
                  </button>
                </div>
              </div>
            </>
          ) : activeTab === 'analytics' && analyticsData ? (
            <div className="cursor-usage-analytics-summary">
              <div className="cursor-usage-analytics-row">
                <span>{t('cursor.usage.periodRange', '时间范围')}</span>
                <strong>{analyticsData.period.startDate} — {analyticsData.period.endDate}</strong>
              </div>
              <div className="cursor-usage-analytics-row">
                <span>{t('cursor.usage.teamMembers', '团队成员数')}</span>
                <strong>{analyticsData.totalMembersInTeam || 1}</strong>
              </div>
              <div className="cursor-usage-analytics-row">
                <span>{t('cursor.usage.dailyMetrics', '每日指标条数')}</span>
                <strong>{analyticsData.dailyMetrics.length}</strong>
              </div>
            </div>
          ) : (
            <p className="modal-muted">{t('cursor.usage.empty', '暂无用量数据')}</p>
          )}
        </div>

        <div className="modal-footer">
          <button className="btn btn-secondary" onClick={onClose}>{t('common.close', '关闭')}</button>
        </div>
      </div>
    </div>
  );
}

interface CursorAccountUsageModalProps {
  isOpen: boolean;
  accountLabel: string;
  accountId: string;
  onClose: () => void;
}

export function CursorAccountUsageModal(props: CursorAccountUsageModalProps) {
  const { isOpen, accountLabel, accountId, onClose } = props;
  const { t } = useTranslation();
  const [selectedPeriod, setSelectedPeriod] = useState<CursorUsagePeriod>('30days');
  const [customStartDate, setCustomStartDate] = useState('');
  const [customEndDate, setCustomEndDate] = useState('');
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [usageData, setUsageData] = useState<CursorAggregatedUsageData | null>(null);
  const [detailsOpen, setDetailsOpen] = useState(false);
  useEscClose(isOpen && !detailsOpen, onClose);

  const dateRange = useMemo(
    () => getCursorUsageDateRange(selectedPeriod, customStartDate, customEndDate),
    [customEndDate, customStartDate, selectedPeriod],
  );

  const periodLabel = useMemo(() => {
    switch (selectedPeriod) {
      case '7days':
        return t('cursor.usage.period7Days', '最近7天');
      case 'thisMonth':
        return t('cursor.usage.periodThisMonth', '本月');
      case 'custom':
        return t('cursor.usage.periodCustom', '自定义');
      default:
        return t('cursor.usage.period30Days', '最近30天');
    }
  }, [selectedPeriod, t]);

  const fetchUsage = useCallback(async (period: CursorUsagePeriod) => {
    if (!accountId) return;
    const range = getCursorUsageDateRange(
      period,
      customStartDate,
      customEndDate,
    );
    if (period === 'custom' && (!customStartDate || !customEndDate)) {
      setError(t('cursor.usage.customDateRequired', '请选择开始和结束日期'));
      return;
    }
    setLoading(true);
    setError(null);
    setUsageData(null);
    try {
      const raw = await cursorService.fetchCursorAggregatedUsage(
        accountId,
        range.startMs,
        range.endMs,
      );
      setUsageData(parseCursorAggregatedUsage(raw));
    } catch (err) {
      setError(String(err));
    } finally {
      setLoading(false);
    }
  }, [accountId, customEndDate, customStartDate, t]);

  useEffect(() => {
    if (!isOpen || !accountId) return;
    setSelectedPeriod('30days');
    setCustomStartDate('');
    setCustomEndDate('');
    setDetailsOpen(false);
    void fetchUsage('30days');
  }, [accountId, isOpen]);

  const handlePeriodChange = (period: CursorUsagePeriod) => {
    setSelectedPeriod(period);
    if (period !== 'custom') {
      void fetchUsage(period);
    }
  };

  if (!isOpen) return null;

  return (
    <>
      <div className="modal-overlay cursor-usage-modal-overlay" onClick={onClose}>
        <div className="modal cursor-usage-modal" onClick={(event) => event.stopPropagation()}>
          <div className="modal-header">
            <div>
              <h2>{t('cursor.usage.title', '账户用量详情')}</h2>
              <p className="modal-subtitle">{accountLabel}</p>
            </div>
            <button className="modal-close" onClick={onClose} aria-label={t('common.close', '关闭')}>
              <X />
            </button>
          </div>

          <div className="modal-body cursor-usage-modal-body">
            <div className="cursor-usage-period-row">
              <span className="cursor-usage-period-label">{t('cursor.usage.periodSelect', '时间段选择')}</span>
              <div className="cursor-usage-period-actions">
                {(['7days', '30days', 'thisMonth', 'custom'] as CursorUsagePeriod[]).map((period) => (
                  <button
                    key={period}
                    type="button"
                    className={`btn btn-secondary btn-sm ${selectedPeriod === period ? 'active' : ''}`}
                    onClick={() => handlePeriodChange(period)}
                  >
                    {period === '7days'
                      ? t('cursor.usage.period7Days', '最近7天')
                      : period === '30days'
                        ? t('cursor.usage.period30Days', '最近30天')
                        : period === 'thisMonth'
                          ? t('cursor.usage.periodThisMonth', '本月')
                          : t('cursor.usage.periodCustom', '自定义')}
                  </button>
                ))}
              </div>
            </div>

            {selectedPeriod === 'custom' ? (
              <div className="cursor-usage-custom-range">
                <label>
                  <span>{t('cursor.usage.startDate', '开始日期')}</span>
                  <input type="date" value={customStartDate} onChange={(e) => setCustomStartDate(e.target.value)} />
                </label>
                <label>
                  <span>{t('cursor.usage.endDate', '结束日期')}</span>
                  <input type="date" value={customEndDate} onChange={(e) => setCustomEndDate(e.target.value)} />
                </label>
                <button
                  type="button"
                  className="btn btn-primary btn-sm"
                  disabled={!customStartDate || !customEndDate}
                  onClick={() => void fetchUsage('custom')}
                >
                  {t('cursor.usage.applyRange', '应用')}
                </button>
              </div>
            ) : null}

            {loading ? (
              <p className="modal-muted">{t('common.loading', '加载中...')}</p>
            ) : error ? (
              <p className="modal-error-text">{error}</p>
            ) : usageData ? (
              <>
                <div className="cursor-usage-summary-head">
                  <h3>{t('cursor.usage.summaryTitle', '用量统计')} — {periodLabel}</h3>
                  <button type="button" className="btn btn-secondary btn-sm" onClick={() => setDetailsOpen(true)}>
                    <ClipboardList size={14} />
                    {t('cursor.usage.viewDetails', '查看明细')}
                  </button>
                </div>

                <div className="cursor-usage-summary-grid">
                  <div className="cursor-usage-summary-card input">
                    <span>{t('cursor.usage.totalInput', '总输入 TOKEN')}</span>
                    <strong>{formatTokenCount(usageData.total_input_tokens)}</strong>
                  </div>
                  <div className="cursor-usage-summary-card output">
                    <span>{t('cursor.usage.totalOutput', '总输出 TOKEN')}</span>
                    <strong>{formatTokenCount(usageData.total_output_tokens)}</strong>
                  </div>
                  <div className="cursor-usage-summary-card cache">
                    <span>{t('cursor.usage.totalCacheRead', '缓存读取 TOKEN')}</span>
                    <strong>{formatTokenCount(usageData.total_cache_read_tokens)}</strong>
                  </div>
                  <div className="cursor-usage-summary-card cost">
                    <span>{t('cursor.usage.totalCost', '总费用')}</span>
                    <strong>{formatCursorUsageDollars(usageData.total_cost_cents)}</strong>
                  </div>
                </div>

                {usageData.aggregations.length > 0 ? (
                  <div className="cursor-usage-model-list">
                    <h4>{t('cursor.usage.modelBreakdown', '模型使用详情')}</h4>
                    {usageData.aggregations.map((model) => (
                      <div key={model.model_intent} className="cursor-usage-model-item">
                        <div className="cursor-usage-model-item-head">
                          <span className="cursor-usage-model-pill">{model.model_intent}</span>
                          <strong>{formatCursorUsageDollars(model.total_cents)}</strong>
                        </div>
                        <div className="cursor-usage-model-item-meta">
                          <span>{t('cursor.usage.inputTokens', '输入')}: {formatTokenCount(model.input_tokens)}</span>
                          <span>{t('cursor.usage.outputTokens', '输出')}: {formatTokenCount(model.output_tokens)}</span>
                          <span>{t('cursor.usage.cacheWriteTokens', '缓存写入')}: {formatTokenCount(model.cache_write_tokens)}</span>
                          <span>{t('cursor.usage.cacheReadTokens', '缓存读取')}: {formatTokenCount(model.cache_read_tokens)}</span>
                        </div>
                      </div>
                    ))}
                  </div>
                ) : null}
              </>
            ) : (
              <p className="modal-muted">{t('cursor.usage.empty', '暂无用量数据')}</p>
            )}
          </div>

          <div className="modal-footer">
            <button className="btn btn-secondary" onClick={onClose}>{t('common.close', '关闭')}</button>
          </div>
        </div>
      </div>

      <CursorUsageDetailsModal
        isOpen={detailsOpen}
        accountId={accountId}
        startMs={dateRange.startMs}
        endMs={dateRange.endMs}
        onClose={() => setDetailsOpen(false)}
      />
    </>
  );
}
