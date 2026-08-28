import { Globe, Monitor, X } from 'lucide-react';
import { useEffect, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useEscClose } from '../hooks/useEscClose';
import type { CursorAuthSession } from '../services/cursorService';

const SESSION_NOTE_MAX_LENGTH = 120;

interface CursorSessionsModalProps {
  isOpen: boolean;
  title: string;
  accountLabel: string;
  sessions: CursorAuthSession[];
  sessionNotes?: Record<string, string> | null;
  loading: boolean;
  revokingSessionId: string | null;
  savingSessionId?: string | null;
  errorMessage?: string | null;
  onRevoke: (sessionId: string) => void;
  onSaveNote: (sessionId: string, note: string) => void | Promise<void>;
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

function SessionRemarkInput(props: {
  sessionId: string;
  value: string;
  disabled: boolean;
  placeholder: string;
  ariaLabel: string;
  onSave: (sessionId: string, note: string) => void | Promise<void>;
}) {
  const { sessionId, value, disabled, placeholder, ariaLabel, onSave } = props;
  const [text, setText] = useState(value);

  useEffect(() => {
    setText(value);
  }, [value]);

  const commit = () => {
    const next = text.trim();
    if (next === value.trim()) return;
    void onSave(sessionId, next);
  };

  return (
    <input
      type="text"
      className="cursor-sessions-remark-input"
      value={text}
      maxLength={SESSION_NOTE_MAX_LENGTH}
      disabled={disabled}
      placeholder={placeholder}
      aria-label={ariaLabel}
      onChange={(event) => setText(event.target.value)}
      onBlur={commit}
      onKeyDown={(event) => {
        if (event.key === 'Enter') {
          event.currentTarget.blur();
        }
      }}
    />
  );
}

export function CursorSessionsModal(props: CursorSessionsModalProps) {
  const {
    isOpen,
    title,
    accountLabel,
    sessions,
    sessionNotes,
    loading,
    revokingSessionId,
    savingSessionId,
    errorMessage,
    onRevoke,
    onSaveNote,
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
          remark: sessionNotes?.[session.sessionId] ?? '',
        };
      }),
    [sessionNotes, sessions, t],
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
          ) : (
            <>
              {errorMessage ? <p className="modal-error-text">{errorMessage}</p> : null}
              {rows.length === 0 ? (
                <p className="modal-muted">{t('cursor.sessions.empty', '暂无活跃会话')}</p>
              ) : (
            <div className="cursor-sessions-table-wrap">
              <table className="cursor-sessions-table">
                <colgroup>
                  <col className="cursor-sessions-col-device" />
                  <col className="cursor-sessions-col-created" />
                  <col className="cursor-sessions-col-action" />
                  <col className="cursor-sessions-col-remark" />
                </colgroup>
                <thead>
                  <tr>
                    <th>{t('cursor.sessions.device', '设备')}</th>
                    <th>{t('cursor.sessions.created', '创建时间')}</th>
                    <th />
                    <th>{t('cursor.sessions.remark', '备注')}</th>
                  </tr>
                </thead>
                <tbody>
                  {rows.map((row) => (
                    <tr key={row.session.sessionId}>
                      <td>
                        <div className="cursor-sessions-device">
                          {row.isWeb ? <Globe size={16} /> : <Monitor size={16} />}
                          <span>{row.typeLabel}</span>
                        </div>
                      </td>
                      <td className="cursor-sessions-created">{row.createdAt}</td>
                      <td className="cursor-sessions-action">
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
                      </td>
                      <td>
                        <SessionRemarkInput
                          sessionId={row.session.sessionId}
                          value={row.remark}
                          disabled={!!revokingSessionId || savingSessionId === row.session.sessionId}
                          placeholder={t('cursor.sessions.remarkPlaceholder', '添加备注')}
                          ariaLabel={t('cursor.sessions.remark', '备注')}
                          onSave={onSaveNote}
                        />
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
              )}
            </>
          )}
        </div>
      </div>
    </div>
  );
}
