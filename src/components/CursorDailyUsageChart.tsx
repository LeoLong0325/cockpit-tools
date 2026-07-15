import { useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { formatCursorUsageDollars } from '../types/cursor';
import type { CursorDailyPaidUsagePoint } from '../types/cursorUsage';

interface CursorDailyUsageChartProps {
  points: CursorDailyPaidUsagePoint[];
}

function formatAxisDollars(cents: number): string {
  if (cents >= 10000) return `$${(cents / 100).toFixed(0)}`;
  if (cents >= 1000) return `$${(cents / 100).toFixed(1)}`;
  return `$${(cents / 100).toFixed(2)}`;
}

/** Lightweight SVG line chart for daily paid-model spend (no chart lib). */
export function CursorDailyUsageChart(props: CursorDailyUsageChartProps) {
  const { points } = props;
  const { t } = useTranslation();
  const [hoverIndex, setHoverIndex] = useState<number | null>(null);

  const chart = useMemo(() => {
    const width = 640;
    const height = 260;
    const pad = { top: 16, right: 16, bottom: 36, left: 52 };
    const innerW = width - pad.left - pad.right;
    const innerH = height - pad.top - pad.bottom;
    const maxCents = Math.max(0, ...points.map((p) => p.costCents));
    const yMax = maxCents <= 0 ? 100 : maxCents * 1.1;
    const n = Math.max(points.length, 1);

    const xAt = (index: number) =>
      pad.left + (n <= 1 ? innerW / 2 : (index / (n - 1)) * innerW);
    const yAt = (cents: number) => pad.top + innerH - (cents / yMax) * innerH;

    const linePath = points
      .map((point, index) => `${index === 0 ? 'M' : 'L'} ${xAt(index).toFixed(1)} ${yAt(point.costCents).toFixed(1)}`)
      .join(' ');

    const areaPath =
      points.length > 0
        ? `${linePath} L ${xAt(points.length - 1).toFixed(1)} ${(pad.top + innerH).toFixed(1)} L ${xAt(0).toFixed(1)} ${(pad.top + innerH).toFixed(1)} Z`
        : '';

    const yTicks = [0, 0.25, 0.5, 0.75, 1].map((ratio) => {
      const cents = yMax * ratio;
      return { cents, y: yAt(cents), label: formatAxisDollars(cents) };
    });

    const labelStep = Math.max(1, Math.ceil(points.length / 8));

    return { width, height, pad, innerW, innerH, xAt, yAt, linePath, areaPath, yTicks, labelStep };
  }, [points]);

  const totalCents = useMemo(
    () => points.reduce((sum, point) => sum + point.costCents, 0),
    [points],
  );
  const totalEvents = useMemo(
    () => points.reduce((sum, point) => sum + point.eventCount, 0),
    [points],
  );
  const peak = useMemo(() => {
    if (points.length === 0) return null;
    return points.reduce((best, point) => (point.costCents > best.costCents ? point : best), points[0]);
  }, [points]);

  const hover = hoverIndex != null ? points[hoverIndex] : null;

  if (points.length === 0 || totalCents <= 0) {
    return <p className="modal-muted">{t('cursor.usage.dailyUsageEmpty', '暂无收费模型每日用量数据')}</p>;
  }

  return (
    <div className="cursor-usage-daily-chart">
      <div className="cursor-usage-daily-chart-head">
        <div>
          <h4>{t('cursor.usage.dailyUsageTitle', '每日收费模型用量')}</h4>
          <p className="cursor-usage-daily-chart-hint">
            {t(
              'cursor.usage.dailyUsageHint',
              '仅统计收费模型（如 Claude），含赠送额度中的同类型消耗；不含 default / Composer。',
            )}
          </p>
        </div>
        <div className="cursor-usage-daily-chart-stats">
          <div>
            <span>{t('cursor.usage.dailyUsageTotal', '合计')}</span>
            <strong>{formatCursorUsageDollars(totalCents)}</strong>
          </div>
          <div>
            <span>{t('cursor.usage.freeCreditEventCount', '使用次数')}</span>
            <strong>{totalEvents.toLocaleString()}</strong>
          </div>
          {peak ? (
            <div>
              <span>{t('cursor.usage.dailyUsagePeak', '峰值日')}</span>
              <strong>
                {peak.label} · {formatCursorUsageDollars(peak.costCents)}
              </strong>
            </div>
          ) : null}
        </div>
      </div>

      <div className="cursor-usage-daily-chart-canvas">
        <svg
          viewBox={`0 0 ${chart.width} ${chart.height}`}
          role="img"
          aria-label={t('cursor.usage.dailyUsageTitle', '每日收费模型用量')}
          onMouseLeave={() => setHoverIndex(null)}
        >
          {chart.yTicks.map((tick) => (
            <g key={tick.label + tick.y}>
              <line
                x1={chart.pad.left}
                x2={chart.pad.left + chart.innerW}
                y1={tick.y}
                y2={tick.y}
                className="cursor-usage-daily-chart-grid"
              />
              <text x={chart.pad.left - 8} y={tick.y + 4} textAnchor="end" className="cursor-usage-daily-chart-axis">
                {tick.label}
              </text>
            </g>
          ))}

          {chart.areaPath ? <path d={chart.areaPath} className="cursor-usage-daily-chart-area" /> : null}
          {chart.linePath ? <path d={chart.linePath} className="cursor-usage-daily-chart-line" fill="none" /> : null}

          {points.map((point, index) => {
            const cx = chart.xAt(index);
            const cy = chart.yAt(point.costCents);
            const active = hoverIndex === index;
            return (
              <g key={point.day}>
                <circle
                  cx={cx}
                  cy={cy}
                  r={active ? 5 : 3.5}
                  className={`cursor-usage-daily-chart-dot${active ? ' is-active' : ''}`}
                />
                {index % chart.labelStep === 0 || index === points.length - 1 ? (
                  <text
                    x={cx}
                    y={chart.height - 12}
                    textAnchor="middle"
                    className="cursor-usage-daily-chart-axis"
                  >
                    {point.label}
                  </text>
                ) : null}
                <rect
                  x={cx - Math.max(chart.innerW / Math.max(points.length, 1) / 2, 8)}
                  y={chart.pad.top}
                  width={Math.max(chart.innerW / Math.max(points.length, 1), 16)}
                  height={chart.innerH}
                  fill="transparent"
                  onMouseEnter={() => setHoverIndex(index)}
                />
              </g>
            );
          })}

          {hover && hoverIndex != null ? (
            <g>
              <line
                x1={chart.xAt(hoverIndex)}
                x2={chart.xAt(hoverIndex)}
                y1={chart.pad.top}
                y2={chart.pad.top + chart.innerH}
                className="cursor-usage-daily-chart-crosshair"
              />
            </g>
          ) : null}
        </svg>

        {hover ? (
          <div className="cursor-usage-daily-chart-tooltip">
            <strong>{hover.day}</strong>
            <span>
              {t('cursor.usage.colCost', '费用')}: {formatCursorUsageDollars(hover.costCents)}
            </span>
            <span>
              {t('cursor.usage.freeCreditEventCount', '使用次数')}: {hover.eventCount.toLocaleString()}
            </span>
          </div>
        ) : null}
      </div>
    </div>
  );
}
