export interface AutoRefreshSchedulerTask {
  key: string;
  label: string;
  intervalMs: number;
  /** 返回 false 表示本次未真正执行（如并发守卫），调度器会尽快重试且不推进周期 */
  run: () => Promise<boolean | void>;
  /** 临时跳过（如全量刷新进行中）；用短间隔重试，不推进完整周期 */
  shouldSkip?: () => boolean;
  /**
   * 首次触发延迟。未指定时默认等于 intervalMs（从调度器启动起精确等待一个完整周期）。
   * 设为 0 可在启动后立即尝试首次执行。
   */
  initialDelayMs?: number;
  /** 并发组：同组内最多同时跑 1 个任务；不同组可并行 */
  concurrencyGroup?: string;
}

export interface AutoRefreshSchedulerOptions {
  tickMs?: number;
  /** 全局最大并发（默认 3，覆盖 antigravity / codex / cursor 等平台） */
  maxConcurrent?: number;
  /** 每个并发组的最大并发（默认 1） */
  maxConcurrentPerGroup?: number;
}

export interface AutoRefreshSchedulerHandle {
  start: () => void;
  stop: () => void;
}

const DEFAULT_TICK_MS = 5_000;
const DEFAULT_MAX_CONCURRENT = 3;
const DEFAULT_MAX_CONCURRENT_PER_GROUP = 1;
const SKIP_RETRY_MS = 5_000;

interface RuntimeTask extends AutoRefreshSchedulerTask {
  nextRunAt: number;
  running: boolean;
}

function clampIntervalMs(intervalMs: number): number {
  return Math.max(intervalMs, DEFAULT_TICK_MS);
}

function buildInitialDelayMs(task: AutoRefreshSchedulerTask, tickMs: number): number {
  if (typeof task.initialDelayMs === 'number' && Number.isFinite(task.initialDelayMs)) {
    if (task.initialDelayMs <= 0) {
      return 0;
    }
    return Math.max(tickMs, Math.floor(task.initialDelayMs));
  }

  return clampIntervalMs(task.intervalMs);
}

function resolveConcurrencyGroup(task: AutoRefreshSchedulerTask): string {
  return task.concurrencyGroup?.trim() || 'default';
}

export function createAutoRefreshScheduler(
  tasks: AutoRefreshSchedulerTask[],
  options: AutoRefreshSchedulerOptions = {},
): AutoRefreshSchedulerHandle {
  const tickMs = Math.max(1_000, options.tickMs ?? DEFAULT_TICK_MS);
  const maxConcurrent = Math.max(1, options.maxConcurrent ?? DEFAULT_MAX_CONCURRENT);
  const maxConcurrentPerGroup = Math.max(
    1,
    options.maxConcurrentPerGroup ?? DEFAULT_MAX_CONCURRENT_PER_GROUP,
  );

  let stopped = false;
  let timerId: number | null = null;
  let activeCount = 0;
  const activeCountByGroup = new Map<string, number>();

  const runtimeTasks: RuntimeTask[] = tasks
    .filter((task) => task.intervalMs > 0)
    .map((task) => ({
      ...task,
      nextRunAt: Date.now() + buildInitialDelayMs(task, tickMs),
      running: false,
    }));

  const getGroupActiveCount = (group: string): number => activeCountByGroup.get(group) ?? 0;

  const incrementGroup = (group: string) => {
    activeCountByGroup.set(group, getGroupActiveCount(group) + 1);
  };

  const decrementGroup = (group: string) => {
    const next = Math.max(0, getGroupActiveCount(group) - 1);
    if (next === 0) {
      activeCountByGroup.delete(group);
    } else {
      activeCountByGroup.set(group, next);
    }
  };

  const canStartTask = (task: RuntimeTask): boolean => {
    if (activeCount >= maxConcurrent) {
      return false;
    }
    const group = resolveConcurrencyGroup(task);
    return getGroupActiveCount(group) < maxConcurrentPerGroup;
  };

  const scheduleDueTasks = () => {
    if (stopped) {
      return;
    }

    const now = Date.now();
    const dueTasks = runtimeTasks
      .filter((task) => !task.running && task.nextRunAt <= now)
      .sort((left, right) => {
        if (left.nextRunAt !== right.nextRunAt) {
          return left.nextRunAt - right.nextRunAt;
        }
        return left.key.localeCompare(right.key);
      });

    for (const task of dueTasks) {
      if (stopped || !canStartTask(task)) {
        break;
      }

      if (task.shouldSkip?.()) {
        task.nextRunAt = now + SKIP_RETRY_MS;
        continue;
      }

      const group = resolveConcurrencyGroup(task);
      task.running = true;
      activeCount += 1;
      incrementGroup(group);

      void Promise.resolve()
        .then(() => task.run())
        .then((executed) => {
          if (executed === false) {
            task.nextRunAt = Date.now();
            return;
          }
          task.nextRunAt = Date.now() + clampIntervalMs(task.intervalMs);
        })
        .catch(() => {
          task.nextRunAt = Date.now() + clampIntervalMs(task.intervalMs);
        })
        .finally(() => {
          task.running = false;
          activeCount = Math.max(0, activeCount - 1);
          decrementGroup(group);
          if (!stopped) {
            scheduleDueTasks();
          }
        });
    }
  };

  return {
    start() {
      if (stopped || timerId !== null || runtimeTasks.length === 0) {
        return;
      }
      scheduleDueTasks();
      timerId = window.setInterval(scheduleDueTasks, tickMs);
    },
    stop() {
      stopped = true;
      if (timerId !== null) {
        window.clearInterval(timerId);
        timerId = null;
      }
    },
  };
}
