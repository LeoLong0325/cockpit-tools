import { useEffect, useId, useLayoutEffect, useMemo, useRef, useState, type CSSProperties } from 'react';
import { createPortal } from 'react-dom';
import { ChevronLeft, ChevronRight } from 'lucide-react';

function pad2(n: number): string {
  return String(n).padStart(2, '0');
}

function toDateValue(date: Date): string {
  return `${date.getFullYear()}-${pad2(date.getMonth() + 1)}-${pad2(date.getDate())}`;
}

function parseDateValue(value: string): Date | null {
  if (!/^\d{4}-\d{2}-\d{2}$/.test(value)) return null;
  const date = new Date(`${value}T00:00:00`);
  return Number.isNaN(date.getTime()) ? null : date;
}

function formatDisplay(value: string): string {
  const date = parseDateValue(value);
  if (!date) return value;
  return `${date.getFullYear()}/${pad2(date.getMonth() + 1)}/${pad2(date.getDate())}`;
}

function startOfMonth(date: Date): Date {
  return new Date(date.getFullYear(), date.getMonth(), 1);
}

function addMonths(date: Date, delta: number): Date {
  return new Date(date.getFullYear(), date.getMonth() + delta, 1);
}

function buildCalendarDays(month: Date): Array<{ date: Date; inMonth: boolean }> {
  const first = startOfMonth(month);
  const startOffset = (first.getDay() + 6) % 7; // Monday-first
  const gridStart = new Date(first);
  gridStart.setDate(first.getDate() - startOffset);

  return Array.from({ length: 42 }, (_, index) => {
    const date = new Date(gridStart);
    date.setDate(gridStart.getDate() + index);
    return {
      date,
      inMonth: date.getMonth() === month.getMonth(),
    };
  });
}

const WEEKDAY_LABELS = Array.from({ length: 7 }, (_, index) => {
  // 2024-01-01 is Monday
  return new Date(2024, 0, 1 + index).toLocaleDateString(undefined, { weekday: 'short' });
});

interface SimpleDateFieldProps {
  value: string;
  onChange: (value: string) => void;
  placeholder?: string;
  'aria-label'?: string;
}

/** In-modal date field: click a day to select and close (avoids native date-picker Esc trap). */
export function SimpleDateField(props: SimpleDateFieldProps) {
  const { value, onChange, placeholder = 'YYYY/MM/DD', 'aria-label': ariaLabel } = props;
  const [open, setOpen] = useState(false);
  const selected = useMemo(() => parseDateValue(value), [value]);
  const [viewMonth, setViewMonth] = useState(() => startOfMonth(selected ?? new Date()));
  const [popoverStyle, setPopoverStyle] = useState<CSSProperties>({});
  const rootRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const popoverRef = useRef<HTMLDivElement>(null);
  const listId = useId();

  useEffect(() => {
    if (!open) return;
    setViewMonth(startOfMonth(selected ?? new Date()));
  }, [open, selected]);

  useLayoutEffect(() => {
    if (!open || !triggerRef.current) return;

    const updatePosition = () => {
      const rect = triggerRef.current?.getBoundingClientRect();
      if (!rect) return;
      const popoverWidth = 280;
      const gap = 6;
      const left = Math.min(
        Math.max(8, rect.left),
        window.innerWidth - popoverWidth - 8,
      );
      const spaceBelow = window.innerHeight - rect.bottom - gap;
      const placeAbove = spaceBelow < 320 && rect.top > spaceBelow;
      setPopoverStyle({
        position: 'fixed',
        top: placeAbove ? undefined : rect.bottom + gap,
        bottom: placeAbove ? window.innerHeight - rect.top + gap : undefined,
        left,
        width: popoverWidth,
        zIndex: 11000,
      });
    };

    updatePosition();
    window.addEventListener('resize', updatePosition);
    window.addEventListener('scroll', updatePosition, true);
    return () => {
      window.removeEventListener('resize', updatePosition);
      window.removeEventListener('scroll', updatePosition, true);
    };
  }, [open]);

  useEffect(() => {
    if (!open) return;
    const onPointerDown = (event: MouseEvent) => {
      const target = event.target as Node;
      if (rootRef.current?.contains(target) || popoverRef.current?.contains(target)) {
        return;
      }
      setOpen(false);
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        event.preventDefault();
        event.stopPropagation();
        setOpen(false);
      }
    };
    document.addEventListener('mousedown', onPointerDown);
    // Capture so Esc closes the calendar before modal useEscClose runs
    window.addEventListener('keydown', onKeyDown, true);
    return () => {
      document.removeEventListener('mousedown', onPointerDown);
      window.removeEventListener('keydown', onKeyDown, true);
    };
  }, [open]);

  const days = useMemo(() => buildCalendarDays(viewMonth), [viewMonth]);
  const monthLabel = `${viewMonth.getFullYear()} / ${pad2(viewMonth.getMonth() + 1)}`;
  const todayValue = toDateValue(new Date());

  const selectDay = (date: Date) => {
    onChange(toDateValue(date));
    setOpen(false);
  };

  return (
    <div className={`simple-date-field${open ? ' is-open' : ''}`} ref={rootRef}>
      <button
        ref={triggerRef}
        type="button"
        className="simple-date-field-trigger"
        aria-label={ariaLabel}
        aria-haspopup="dialog"
        aria-expanded={open}
        aria-controls={listId}
        onClick={() => setOpen((prev) => !prev)}
      >
        {value ? formatDisplay(value) : <span className="simple-date-field-placeholder">{placeholder}</span>}
      </button>

      {open
        ? createPortal(
            <div
              ref={popoverRef}
              className="simple-date-field-popover"
              id={listId}
              role="dialog"
              aria-label={ariaLabel}
              style={popoverStyle}
              onMouseDown={(event) => event.stopPropagation()}
              onClick={(event) => event.stopPropagation()}
            >
              <div className="simple-date-field-month-row">
                <button
                  type="button"
                  className="simple-date-field-nav"
                  aria-label="Previous month"
                  onClick={() => setViewMonth((prev) => addMonths(prev, -1))}
                >
                  <ChevronLeft size={16} />
                </button>
                <span className="simple-date-field-month-label">{monthLabel}</span>
                <button
                  type="button"
                  className="simple-date-field-nav"
                  aria-label="Next month"
                  onClick={() => setViewMonth((prev) => addMonths(prev, 1))}
                >
                  <ChevronRight size={16} />
                </button>
              </div>

              <div className="simple-date-field-weekdays">
                {WEEKDAY_LABELS.map((label) => (
                  <span key={label}>{label}</span>
                ))}
              </div>

              <div className="simple-date-field-grid">
                {days.map(({ date, inMonth }) => {
                  const dayValue = toDateValue(date);
                  const isSelected = dayValue === value;
                  const isToday = dayValue === todayValue;
                  return (
                    <button
                      key={`${dayValue}-${inMonth ? 'in' : 'out'}`}
                      type="button"
                      className={[
                        'simple-date-field-day',
                        inMonth ? '' : 'is-outside',
                        isSelected ? 'is-selected' : '',
                        isToday ? 'is-today' : '',
                      ]
                        .filter(Boolean)
                        .join(' ')}
                      onClick={() => selectDay(date)}
                    >
                      {date.getDate()}
                    </button>
                  );
                })}
              </div>
            </div>,
            document.body,
          )
        : null}
    </div>
  );
}
