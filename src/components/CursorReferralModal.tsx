import { Check, Copy, X } from 'lucide-react';
import { useCallback, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useEscClose } from '../hooks/useEscClose';
import type { CursorReferralStatus } from '../types/cursor';
import { formatCursorUsageDollars } from '../types/cursor';

interface CursorReferralModalProps {
  isOpen: boolean;
  title: string;
  accountLabel: string;
  status: CursorReferralStatus | null;
  loading: boolean;
  errorMessage?: string | null;
  onClose: () => void;
}

function formatReferralCount(
  earned: number | null,
  max: number | null,
): string {
  if (earned == null && max == null) return '—';
  if (max != null && earned != null) return `${earned} / ${max}`;
  if (earned != null) return String(earned);
  return '—';
}

export function CursorReferralModal(props: CursorReferralModalProps) {
  const {
    isOpen,
    title,
    accountLabel,
    status,
    loading,
    errorMessage,
    onClose,
  } = props;
  const { t } = useTranslation();
  const [copiedField, setCopiedField] = useState<'code' | 'link' | null>(null);
  useEscClose(isOpen, onClose);

  const copyText = useCallback(async (text: string, field: 'code' | 'link') => {
    if (!text) return;
    try {
      await navigator.clipboard.writeText(text);
      setCopiedField(field);
      window.setTimeout(() => setCopiedField(null), 1200);
    } catch {
      // ignore clipboard errors
    }
  }, []);

  if (!isOpen) return null;

  const cycleCredit = status?.creditEarnedThisCycleCents ?? null;
  const lifetimeCredit =
    status?.lifetimeCreditEarnedCents ?? status?.creditEarnedThisCycleCents ?? null;

  return (
    <div className="modal-overlay">
      <div className="modal cursor-referral-modal" onClick={(event) => event.stopPropagation()}>
        <div className="modal-header">
          <div>
            <h2>{title}</h2>
            <p className="modal-subtitle">{accountLabel}</p>
          </div>
          <button className="modal-close" onClick={onClose} aria-label={t('common.close', '关闭')}>
            <X />
          </button>
        </div>

        <div className="modal-body">
          {loading ? (
            <p className="modal-muted">{t('common.loading', '加载中...')}</p>
          ) : errorMessage ? (
            <p className="modal-error-text">{errorMessage}</p>
          ) : !status ? (
            <p className="modal-muted">{t('cursor.referral.empty', '暂无邀请奖励数据')}</p>
          ) : (
            <div className="cursor-referral-table-wrap">
              <table className="cursor-referral-table">
                <thead>
                  <tr>
                    <th>{t('cursor.referral.code', '邀请码')}</th>
                    <th>{t('cursor.referral.invitedCount', '本周期邀请人数')}</th>
                    <th>{t('cursor.referral.rewardAmount', '已获得奖励')}</th>
                  </tr>
                </thead>
                <tbody>
                  <tr>
                    <td>
                      <div className="cursor-referral-cell">
                        <span className="cursor-referral-code">
                          {status.referralCode || '—'}
                        </span>
                        {status.referralCode ? (
                          <button
                            type="button"
                            className="cursor-referral-copy-btn"
                            onClick={() => copyText(status.referralCode || '', 'code')}
                            title={t('common.copy', '复制')}
                          >
                            {copiedField === 'code' ? <Check size={14} /> : <Copy size={14} />}
                          </button>
                        ) : null}
                      </div>
                    </td>
                    <td>
                      {formatReferralCount(
                        status.rewardsEarnedThisCycle,
                        status.maxRewardsPerCycle,
                      )}
                    </td>
                    <td>
                      <div className="cursor-referral-reward-cell">
                        <span>{formatCursorUsageDollars(cycleCredit ?? lifetimeCredit)}</span>
                        {lifetimeCredit != null &&
                        cycleCredit != null &&
                        lifetimeCredit !== cycleCredit ? (
                          <span className="cursor-referral-subtext">
                            {t('cursor.referral.lifetimeReward', '累计')}:{' '}
                            {formatCursorUsageDollars(lifetimeCredit)}
                          </span>
                        ) : null}
                      </div>
                    </td>
                  </tr>
                </tbody>
              </table>

              {status.referralLink ? (
                <div className="cursor-referral-link-row">
                  <span className="cursor-referral-link-label">
                    {t('cursor.referral.link', '邀请链接')}
                  </span>
                  <code className="cursor-referral-link">{status.referralLink}</code>
                  <button
                    type="button"
                    className="btn btn-secondary btn-sm"
                    onClick={() => copyText(status.referralLink || '', 'link')}
                  >
                    {copiedField === 'link' ? (
                      <>
                        <Check size={14} />
                        {t('cursor.referral.copied', '已复制')}
                      </>
                    ) : (
                      <>
                        <Copy size={14} />
                        {t('cursor.referral.copyLink', '复制链接')}
                      </>
                    )}
                  </button>
                </div>
              ) : null}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
