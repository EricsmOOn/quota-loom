import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
} from "react";
import { LogicalSize } from "@tauri-apps/api/dpi";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  Braces,
  CalendarClock,
  ExternalLink,
  Minus,
  Sparkles,
} from "lucide-react";
import {
  hideFloatingWindow,
  isDesktopRuntime,
  showDashboardWindow,
} from "../lib/api";
import { formatPercent, formatResetTime, formatTokens } from "../lib/format";
import { floatingMeterPercent } from "../lib/progress";
import { loadSourceFilter, useUsage, useWeeklyUsage } from "../hooks/useUsage";

type FlowPhase = "idle" | "arrive" | "transfer" | "depart";

interface FlowMetricProps {
  value: number | null;
  label: string;
  ariaLabel: string;
  className?: string;
  formatValue: (value: number) => string;
  formatDelta: (value: number) => string;
  unavailableText?: string;
}

function FlowMetric({
  value,
  label,
  ariaLabel,
  className = "",
  formatValue,
  formatDelta,
  unavailableText = "—",
}: FlowMetricProps) {
  const [displayValue, setDisplayValue] = useState(value);
  const [deltaValue, setDeltaValue] = useState(0);
  const [direction, setDirection] = useState<"up" | "down">("up");
  const [phase, setPhase] = useState<FlowPhase>("idle");
  const [animationId, setAnimationId] = useState(0);
  const displayedRef = useRef(value);
  const latestTargetRef = useRef(value);
  const pendingTargetRef = useRef<number | null | undefined>(undefined);
  const runningRef = useRef(false);
  const frameRef = useRef<number | null>(null);
  const timersRef = useRef<number[]>([]);
  const metricRef = useRef<HTMLDivElement>(null);
  const mainValueRef = useRef<HTMLElement>(null);

  const clearMotion = useCallback(() => {
    if (frameRef.current !== null) cancelAnimationFrame(frameRef.current);
    timersRef.current.forEach(window.clearTimeout);
    frameRef.current = null;
    timersRef.current = [];
  }, []);

  const runAnimation = useCallback(function animate(from: number, to: number) {
    const difference = to - from;
    if (difference === 0) return;

    runningRef.current = true;
    setAnimationId((current) => current + 1);
    setDirection(difference > 0 ? "up" : "down");
    setDeltaValue(Math.abs(difference));
    setPhase("arrive");

    const transferTimer = window.setTimeout(() => {
      const startedAt = performance.now();
      setPhase("transfer");

      const tick = (now: number) => {
        const progress = Math.min(1, (now - startedAt) / 1_080);
        const eased = 1 - Math.pow(1 - progress, 3);
        const nextValue = from + difference * eased;
        displayedRef.current = nextValue;
        setDisplayValue(nextValue);

        if (progress < 1) {
          frameRef.current = requestAnimationFrame(tick);
          return;
        }

        displayedRef.current = to;
        setDisplayValue(to);
        setPhase("depart");

        const departTimer = window.setTimeout(() => {
          setPhase("idle");
          runningRef.current = false;
          const pendingTarget = pendingTargetRef.current;
          pendingTargetRef.current = undefined;
          if (
            pendingTarget !== undefined &&
            pendingTarget !== null &&
            pendingTarget !== to
          ) {
            animate(to, pendingTarget);
          } else if (pendingTarget === null) {
            displayedRef.current = null;
            setDisplayValue(null);
          }
        }, 460);
        timersRef.current.push(departTimer);
      };

      frameRef.current = requestAnimationFrame(tick);
    }, 680);
    timersRef.current.push(transferTimer);
  }, []);

  useEffect(() => {
    if (latestTargetRef.current === value) return;
    latestTargetRef.current = value;

    if (value === null || displayedRef.current === null) {
      clearMotion();
      runningRef.current = false;
      pendingTargetRef.current = undefined;
      displayedRef.current = value;
      setDisplayValue(value);
      setPhase("idle");
      return;
    }

    if (runningRef.current) {
      pendingTargetRef.current = value;
      return;
    }
    runAnimation(displayedRef.current, value);
  }, [clearMotion, runAnimation, value]);

  useEffect(() => clearMotion, [clearMotion]);

  useLayoutEffect(() => {
    if (!metricRef.current || !mainValueRef.current) return;
    const shift =
      Math.ceil(mainValueRef.current.getBoundingClientRect().width / 2) + 6;
    metricRef.current.style.setProperty("--main-shift", `-${shift}px`);
  }, [animationId]);

  return (
    <div
      ref={metricRef}
      className={`flow-metric ${phase} ${className}`}
      aria-label={ariaLabel}
    >
      <small className="metric-label" aria-hidden="true">
        {label}
      </small>
      <strong ref={mainValueRef}>
        {displayValue === null ? unavailableText : formatValue(displayValue)}
      </strong>
      {phase !== "idle" && (
        <span
          key={animationId}
          className={`metric-delta ${phase} ${direction}`}
          aria-hidden="true"
        >
          <b>{direction === "up" ? "+" : "↓"}</b>
          {formatDelta(deltaValue)}
        </span>
      )}
    </div>
  );
}

function formatFloatingTokens(value: number) {
  const rounded = Math.max(0, value);
  if (rounded >= 1_000_000_000)
    return `${(rounded / 1_000_000_000).toFixed(1)}B`;
  if (rounded >= 1_000_000) return `${(rounded / 1_000_000).toFixed(1)}M`;
  if (rounded >= 1_000) return `${(rounded / 1_000).toFixed(1)}K`;
  return rounded.toFixed(1);
}

function formatFloatingCost(value: number) {
  return `$${Math.max(0, value).toFixed(2)}`;
}

export function FloatingPanel() {
  const [source] = useState(loadSourceFilter);
  const { data, isLoading, isError } = useUsage("today", source);
  const weeklyUsage = useWeeklyUsage().data;
  const codexFamilyFilter = source === "codexCli" || source === "chatGptCodex";
  const quota = data?.quotaEstimate ?? null;
  const summary = data?.summary;
  const meterPercent = quota
    ? Math.max(0, Math.min(100, quota.remainingPercent))
    : floatingMeterPercent(
        weeklyUsage?.remainingPercent ?? null,
        summary?.cacheHitRate ?? 0,
      );
  const meterLabel = quota
    ? `今日额度剩余 ${Math.round(quota.remainingPercent)}%（估算）`
    : weeklyUsage && codexFamilyFilter
      ? `周额度剩余 ${Math.round(weeklyUsage.remainingPercent)}%`
      : `缓存命中率 ${formatPercent(summary?.cacheHitRate ?? 0)}`;
  const hasQuotaRow = Boolean(quota || weeklyUsage);

  useEffect(() => {
    if (!isDesktopRuntime() || !hasQuotaRow) return;
    void getCurrentWindow().setSize(new LogicalSize(420, 162));
  }, [hasQuotaRow]);

  function beginDrag(event: React.MouseEvent) {
    if (
      event.button !== 0 ||
      !isDesktopRuntime() ||
      (event.target as HTMLElement).closest("button")
    )
      return;
    void getCurrentWindow().startDragging();
  }

  return (
    <main className="floating-shell">
      <section className="floating-panel">
        <header className="floating-header" onMouseDown={beginDrag}>
          <div>
            <Braces size={14} />
            <strong>{data?.sourceLabel ?? "AI CODE"}</strong>
            <span className={isError ? "live-pip fault" : "live-pip"} />
          </div>
          <div className="floating-actions">
            <button onClick={() => void showDashboardWindow()} title="打开统计">
              <ExternalLink size={13} />
            </button>
            <button onClick={() => void hideFloatingWindow()} title="隐藏">
              <Minus size={14} />
            </button>
          </div>
        </header>
        {isLoading ? (
          <div className="floating-loading">
            <i />
            <span>CALIBRATING LOCAL DATA</span>
          </div>
        ) : (
          <>
            <div className="floating-primary">
              <FlowMetric
                value={summary?.totalTokens ?? 0}
                label="TOKEN"
                ariaLabel="今日 Token"
                formatValue={formatFloatingTokens}
                formatDelta={formatFloatingTokens}
              />
              <FlowMetric
                value={
                  summary?.estimatedCostUsd === null ||
                  summary?.estimatedCostUsd === undefined
                    ? null
                    : Number(summary.estimatedCostUsd)
                }
                label={summary?.unpricedModels ? "COST*" : "COST"}
                ariaLabel={
                  summary?.unpricedModels
                    ? "预计费用，仅包含已定价模型"
                    : "预计费用"
                }
                className="cost"
                formatValue={formatFloatingCost}
                formatDelta={formatFloatingCost}
                unavailableText="未定价"
              />
            </div>
            <div
              className="floating-meter"
              role="progressbar"
              aria-label={meterLabel}
              aria-valuemin={0}
              aria-valuemax={100}
              aria-valuenow={Math.round(meterPercent)}
            >
              <i style={{ width: `${meterPercent}%` }} />
            </div>
            {quota ? (
              <div
                className="floating-weekly"
                aria-label={`今日额度剩余 ${Math.round(quota.remainingPercent)}%（估算）`}
              >
                <span>
                  <CalendarClock size={13} /> DAILY LEFT
                  <strong>{Math.round(quota.remainingPercent)}%</strong>
                </span>
                <time>RESET {formatResetTime(quota.resetsAt)}</time>
              </div>
            ) : weeklyUsage && codexFamilyFilter ? (
              <div
                className="floating-weekly"
                aria-label={`周额度剩余 ${Math.round(weeklyUsage.remainingPercent)}%`}
              >
                <span>
                  <CalendarClock size={13} /> WEEK LEFT
                  <strong>{Math.round(weeklyUsage.remainingPercent)}%</strong>
                </span>
                {weeklyUsage.resetsAt ? (
                  <time>RESET {formatResetTime(weeklyUsage.resetsAt)}</time>
                ) : null}
              </div>
            ) : null}
            <footer className="floating-footer">
              <span>
                <Sparkles size={13} /> CACHE{" "}
                {formatPercent(summary?.cacheHitRate ?? 0)}
              </span>
              <span>
                IN{" "}
                {formatTokens(
                  (summary?.freshInputTokens ?? 0) +
                    (summary?.cachedInputTokens ?? 0),
                )}
              </span>
              <span>OUT {formatTokens(summary?.outputTokens ?? 0)}</span>
              <span>{summary?.calls ?? 0} CALLS</span>
            </footer>
          </>
        )}
      </section>
    </main>
  );
}
