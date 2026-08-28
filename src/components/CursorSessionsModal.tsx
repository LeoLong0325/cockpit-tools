import { Globe, Monitor, X } from 'lucide-react';
import { useMemo } from 'react';
import { useTranslation } from 'react-i18next';
import { useEscClose } from '../hooks/useEscClose';
import type { CursorAuthSession } from '../services/cursorService';

interface CursorSessionsModalProps {
  isOpen: boolean;
  title: string;
  accountLabel: string;
  sessions: CursorAuthSession[];
  loading: boolean;
  revokingSessionId: string | null;
  errorMessage?: string | null;
  onRevoke: (sessionId: string) => void;
  onClose: () => void;
}

function parseSessionDate(value: string): Date | null {
  if (!value.trim()) return null;
  const parsed = new Date(value);
  return Number.isNaN(parsed.getTime()) ? null : parsed;
}

function formatCreatedAt(date: Date): string {
  const pad = (value: number) => String(value).padStart(2, '0');
  return `${pad(date.getMonth() + 1)}/${pad(date.getDate())} ${pad(date.getHours())}:${pad(date.getMinutes())}:${pad(date.getSeconds())}`;
}

export function CursorSessionsModal(props: CursorSessionsModalProps) {
  const {
    isOpen,
    title,
    accountLabel,
    sessions,
    loading,
    revokingSessionId,
    errorMessage,
    onRevoke,
    onClose,
  } = props;
  const { t } = useTranslation();
  useEscClose(isOpen, onClose);

  const rows = useMemo(
    () =>
      sessions.map((session) => {
        const created = parseSessionDate(session.createdAt);
        const typeKey = (session.type || '').toUpperCase();
        const isWeb = typeKey.includes('WEB');
        const isDesktop = typeKey.includes('CLIENT') || typeKey.includes('DESKTOP');
        return {
          session,
          isWeb,
          typeLabel: isWeb
            ? t('cursor.sessions.typeWeb', 'Web')
            : isDesktop
              ? t('cursor.sessions.typeDesktop', 'Desktop App')
              : (session.type || t('cursor.sessions.typeUnknown', '未知设备')).replace(
                  /^SESSION_TYPE_/,
                  '',
                ),
          createdAt: created ? formatCreatedAt(created) : '—',
        };
      }),
    [sessions, t],
  );

  if (!isOpen) return null;

  return (
    <div className="modal-overlay">
      <div className="modal cursor-sessions-modal" onClick={(event) => event.stopPropagation()}>
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
          ) : rows.length === 0 ? (
            <p className="modal-muted">{t('cursor.sessions.empty', '暂无活跃会话')}</p>
          ) : (
            <div className="cursor-sessions-table-wrap">
              <div className="cursor-sessions-head">
                <span>{t('cursor.sessions.device', '设备')}</span>
                <span>{t('cursor.sessions.created', '创建时间')}</span>
                <span />
              </div>
              {rows.map((row) => (
                <div key={row.session.sessionId} className="cursor-sessions-row">
                  <div className="cursor-sessions-device">
                    {row.isWeb ? <Globe size={16} /> : <Monitor size={16} />}
                    <span>{row.typeLabel}</span>
                  </div>
                  <div className="cursor-sessions-created">{row.createdAt}</div>
                  <button
                    type="button"
                    className="cursor-sessions-revoke"
                    disabled={!!revokingSessionId}
                    onClick={() => onRevoke(row.session.sessionId)}
                  >
                    {revokingSessionId === row.session.sessionId
                      ? t('common.loading', '加载中...')
                      : t('cursor.sessions.revoke', '撤销')}
                  </button>
                </div>
              ))}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
