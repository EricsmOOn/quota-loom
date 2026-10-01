import { useEffect, useState } from "react";
import {
  Activity,
  ArrowDownRight,
  ArrowUpRight,
  Box,
  Braces,
  CalendarClock,
  Coins,
  RotateCcw,
  Save,
  Settings2,
  Share2,
  Sparkles,
  X,
} from "lucide-react";
import {
  Area,
  CartesianGrid,
  ComposedChart,
  Line,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
} from "recharts";
import {
  chooseSourceHome,
  getUsedModelPrices,
  refreshModelsDevPrices,
  removeSourceHome,
  resetUsageCache,
  syncUsage,
  updateModelPrice,
} from "../lib/api";
import {
  formatCost,
  formatInteger,
  formatPercent,
  formatResetTime,
  formatTime,
  formatTokens,
} from "../lib/format";
import {
  loadSourceFilter,
  saveSourceFilter,
  useSourceHomes,
  useUsage,
  useUsageEvents,
  useWeeklyUsage,
} from "../hooks/useUsage";
import { ShareCard } from "./ShareCard";
import type {
  ModelPriceEntry,
  RangePreset,
  SourceFilter,
  UsageTrendPoint,
} from "../types";

type TrendChartPoint = UsageTrendPoint & {
  costUsd: number | null;
};

const rangeOptions: Array<{ value: RangePreset; label: string }> = [
  { value: "today", label: "今日" },
  { value: "24h", label: "24H" },
  { value: "7d", label: "7D" },
  { value: "30d", label: "30D" },
];

export function Dashboard() {
  const [preset, setPreset] = useState<RangePreset>("today");
  const [source, setSource] = useState<SourceFilter>(loadSourceFilter);
  const [managingSources, setManagingSources] = useState(false);
  const [priceEditorOpen, setPriceEditorOpen] = useState(false);
  const [shareCardOpen, setShareCardOpen] = useState(false);
  const [resettingCache, setResettingCache] = useState(false);
  useUsageEvents();
  const query = useUsage(preset, source);
  const homes = useSourceHomes().data ?? [];
  const weeklyUsage = useWeeklyUsage().data;
  const snapshot = query.data;
  const trendData = snapshot ? normalizeTrendCosts(snapshot.trends) : [];
  const hasCost = trendData.some((point) => point.costUsd !== null);
  const codexFamilyFilter = source === "codexCli" || source === "chatGptCodex";

  function pickSource(next: SourceFilter) {
    setSource(next);
    saveSourceFilter(next);
  }

  async function refresh() {
    await syncUsage();
    await query.refetch();
  }

  async function resetCache() {
    const confirmed = window.confirm(
      `清空本地统计缓存并从 ${snapshot?.sourceLabel ?? "当前"} 日志重新构建？`,
    );
    if (!confirmed) return;
    setResettingCache(true);
    try {
      await resetUsageCache();
      await query.refetch();
    } catch (error) {
      window.alert(`重置缓存失败：${String(error)}`);
    } finally {
      setResettingCache(false);
    }
  }

  return (
    <main className="dashboard-shell">
      <div className="ambient-grid" aria-hidden="true" />
      <header className="topbar reveal reveal-1">
        <div className="brand-lockup">
          <div className="brand-mark">
            <Braces size={19} strokeWidth={1.8} />
          </div>
          <div>
            <strong>{snapshot?.sourceBrand ?? "CODEX CLI / USAGE"}</strong>
            <span>LOCAL TELEMETRY INSTRUMENT · V{__APP_VERSION__}</span>
          </div>
        </div>
        <div className="topbar-actions">
          <button
            className="source-chip"
            title="管理数据目录"
            onClick={() => setManagingSources(true)}
          >
            <span
              className={query.isError ? "status-dot fault" : "status-dot"}
            />
            {homes.length > 0
              ? `${homes.length} 个数据目录`
              : (snapshot?.codexHome ?? "正在定位数据目录")}
          </button>
          <button
            className="price-settings-button"
            onClick={() => setShareCardOpen(true)}
          >
            <Share2 size={15} />
            分享卡
          </button>
          <button
            className="price-settings-button"
            onClick={() => setPriceEditorOpen(true)}
          >
            <Settings2 size={15} />
            模型价格
          </button>
          <button
            className="reset-cache-button"
            onClick={() => void resetCache()}
            disabled={resettingCache}
          >
            <RotateCcw size={15} className={resettingCache ? "spin" : ""} />
            {resettingCache ? "重建中" : "重置缓存"}
          </button>
        </div>
      </header>

      <section className="range-row reveal reveal-2">
        <div className="source-switch" role="group" aria-label="统计来源">
          <button
            className={source === "all" ? "active" : ""}
            onClick={() => pickSource("all")}
          >
            全部
          </button>
          {homes
            .filter(
              (home, index) =>
                homes.findIndex((other) => other.kind === home.kind) === index,
            )
            .map((home) => (
              <button
                key={home.kind}
                className={source === home.kind ? "active" : ""}
                onClick={() => pickSource(home.kind)}
              >
                {home.label}
              </button>
            ))}
        </div>
        <div>
          <p className="eyebrow">OBSERVATION WINDOW</p>
        </div>
        <div className="range-switch" role="group" aria-label="统计时间范围">
          {rangeOptions.map((option) => (
            <button
              key={option.value}
              className={preset === option.value ? "active" : ""}
              onClick={() => setPreset(option.value)}
            >
              {option.label}
            </button>
          ))}
        </div>
      </section>

      {query.isLoading ? (
        <LoadingState />
      ) : query.isError ? (
        <ErrorState onRetry={refresh} />
      ) : snapshot ? (
        <>
          <section
            className={`instrument-grid reveal reveal-3${
              (weeklyUsage && codexFamilyFilter) || snapshot.quotaEstimate
                ? " has-weekly"
                : ""
            }`}
          >
            <article className="hero-instrument">
              <div className="instrument-label">
                <Activity size={15} /> TOTAL PROCESSED
              </div>
              <div className="hero-number">
                {formatTokens(snapshot.summary.totalTokens)}
              </div>
              <div className="hero-unit">
                TOKENS /{" "}
                {rangeOptions.find((option) => option.value === preset)?.label}
              </div>
              <div className="hero-footer">
                <span>
                  <i className="swatch fresh" />
                  新增输入 {formatTokens(snapshot.summary.freshInputTokens)}
                </span>
                <span>
                  <i className="swatch cached" />
                  缓存输入 {formatTokens(snapshot.summary.cachedInputTokens)}
                </span>
                <span>
                  <i className="swatch output" />
                  模型输出 {formatTokens(snapshot.summary.outputTokens)}
                </span>
              </div>
            </article>

            <article className="metric-instrument cost">
              <div className="instrument-label">
                <Coins size={15} /> ESTIMATED COST
              </div>
              <strong>
                {formatCost(snapshot.summary.estimatedCostUsd, 1)}
              </strong>
              <span>
                {snapshot.summary.unpricedModels > 0
                  ? snapshot.summary.estimatedCostUsd === null
                    ? `${snapshot.summary.unpricedModels} 个模型未配置价格`
                    : `已定价部分 · ${snapshot.summary.unpricedModels} 个模型未定价`
                  : "按当前模型价格估算"}
              </span>
            </article>

            {weeklyUsage && codexFamilyFilter ? (
              <article className="metric-instrument weekly">
                <div className="instrument-label">
                  <CalendarClock size={15} /> WEEKLY REMAINING
                </div>
                <strong>{Math.round(weeklyUsage.remainingPercent)}%</strong>
                <span>账户周额度剩余</span>
                <div className="weekly-progress" aria-hidden="true">
                  <i style={{ width: `${weeklyUsage.remainingPercent}%` }} />
                </div>
                {weeklyUsage.resetsAt ? (
                  <small>{formatResetTime(weeklyUsage.resetsAt)} 重置</small>
                ) : null}
              </article>
            ) : null}

            {snapshot.quotaEstimate ? (
              <article className="metric-instrument weekly">
                <div className="instrument-label">
                  <CalendarClock size={15} /> DAILY QUOTA · EST
                </div>
                <strong>
                  {Math.round(snapshot.quotaEstimate.remainingPercent)}%
                </strong>
                <span>
                  {snapshot.quotaEstimate.planName} · 今日已用{" "}
                  {formatTokens(snapshot.quotaEstimate.usedTokens)}
                </span>
                <div className="weekly-progress" aria-hidden="true">
                  <i
                    style={{
                      width: `${Math.min(
                        100,
                        snapshot.quotaEstimate.remainingPercent,
                      )}%`,
                    }}
                  />
                </div>
                <small>
                  {formatResetTime(snapshot.quotaEstimate.resetsAt)} 重置 ·
                  平台口径估算
                </small>
              </article>
            ) : null}

            <article className="metric-instrument cache">
              <div className="instrument-label">
                <Sparkles size={15} /> CACHE EFFICIENCY
              </div>
              <div
                className="cache-gauge"
                style={
                  {
                    "--cache-rate": `${snapshot.summary.cacheHitRate * 360}deg`,
                  } as React.CSSProperties
                }
              >
                <strong>{formatPercent(snapshot.summary.cacheHitRate)}</strong>
                <span>HIT RATE</span>
              </div>
            </article>

            <article className="metric-instrument calls">
              <div className="instrument-label">
                <Box size={15} /> LOCAL ACTIVITY
              </div>
              <div className="split-reading">
                <div>
                  <strong>{formatInteger(snapshot.summary.calls)}</strong>
                  <span>调用</span>
                </div>
                <div>
                  <strong>{formatInteger(snapshot.summary.threads)}</strong>
                  <span>会话</span>
                </div>
              </div>
              <small>{formatTime(snapshot.generatedAt, true)} 校准</small>
            </article>
          </section>

          <section className="analytics-grid reveal reveal-4">
            <article className="panel trend-panel">
              <PanelHeading
                code="A01"
                title="Token 趋势"
                meta={`${snapshot.trends.length} 桶 · COST 同步分段`}
              />
              <div className="chart-wrap">
                <ResponsiveContainer width="100%" height="100%">
                  <ComposedChart
                    data={trendData}
                    margin={{ top: 18, right: 0, bottom: 0, left: -14 }}
                  >
                    <defs>
                      <linearGradient
                        id="cachedArea"
                        x1="0"
                        y1="0"
                        x2="0"
                        y2="1"
                      >
                        <stop
                          offset="0%"
                          stopColor="#8bc6ad"
                          stopOpacity={0.7}
                        />
                        <stop
                          offset="100%"
                          stopColor="#8bc6ad"
                          stopOpacity={0.08}
                        />
                      </linearGradient>
                      <linearGradient
                        id="outputArea"
                        x1="0"
                        y1="0"
                        x2="0"
                        y2="1"
                      >
                        <stop
                          offset="0%"
                          stopColor="#efc84a"
                          stopOpacity={0.75}
                        />
                        <stop
                          offset="100%"
                          stopColor="#efc84a"
                          stopOpacity={0.08}
                        />
                      </linearGradient>
                    </defs>
                    <CartesianGrid
                      vertical={false}
                      stroke="#111111"
                      strokeOpacity={0.18}
                      strokeDasharray="4 4"
                    />
                    <XAxis
                      dataKey="bucketStart"
                      tickFormatter={(value) =>
                        formatTime(
                          Number(value),
                          preset !== "today" && preset !== "24h",
                        )
                      }
                      tick={{ fill: "#16335f", fontSize: 12, fontWeight: 600 }}
                      axisLine={false}
                      tickLine={false}
                    />
                    <YAxis
                      yAxisId="tokens"
                      tickFormatter={(value) => formatTokens(Number(value))}
                      tick={{ fill: "#16335f", fontSize: 12, fontWeight: 600 }}
                      axisLine={false}
                      tickLine={false}
                    />
                    {hasCost ? (
                      <YAxis
                        yAxisId="cost"
                        orientation="right"
                        width={46}
                        tickFormatter={(value) =>
                          `$${Number(value).toFixed(1)}`
                        }
                        tick={{
                          fill: "#8e0c24",
                          fontSize: 11,
                          fontWeight: 600,
                        }}
                        axisLine={false}
                        tickLine={false}
                      />
                    ) : null}
                    <Tooltip content={<TrendTooltip />} />
                    <Area
                      yAxisId="tokens"
                      type="linear"
                      dataKey="cachedInputTokens"
                      stackId="tokens"
                      stroke="#111111"
                      fill="url(#cachedArea)"
                      strokeWidth={2}
                    />
                    <Area
                      yAxisId="tokens"
                      type="linear"
                      dataKey="freshInputTokens"
                      stackId="tokens"
                      stroke="#16335f"
                      fill="rgba(73,118,182,.48)"
                      strokeWidth={2}
                    />
                    <Area
                      yAxisId="tokens"
                      type="linear"
                      dataKey="outputTokens"
                      stackId="tokens"
                      stroke="#9b7900"
                      fill="url(#outputArea)"
                      strokeWidth={2}
                    />
                    {hasCost ? (
                      <Line
                        yAxisId="cost"
                        type="stepAfter"
                        dataKey="costUsd"
                        stroke="#8e0c24"
                        strokeWidth={3}
                        strokeDasharray="7 4"
                        dot={false}
                        connectNulls
                        isAnimationActive={false}
                      />
                    ) : null}
                  </ComposedChart>
                </ResponsiveContainer>
              </div>
            </article>

            <article className="panel composition-panel">
              <PanelHeading
                code="A02"
                title="输入构成"
                meta="CACHE NORMALIZED"
              />
              <TokenComposition summary={snapshot.summary} />
            </article>
          </section>

          <section className="detail-grid reveal reveal-5">
            <article className="panel model-panel">
              <PanelHeading
                code="B01"
                title="模型分布"
                meta={`${snapshot.models.length} MODELS`}
              />
              <div className="model-table">
                <div className="table-row table-head">
                  <span>MODEL</span>
                  <span>TOKENS</span>
                  <span>CACHE</span>
                  <span>CALLS</span>
                  <span>COST</span>
                </div>
                {snapshot.models.length === 0 ? (
                  <EmptyLine />
                ) : (
                  snapshot.models.map((model) => (
                    <div className="table-row" key={model.model}>
                      <span className="model-name">
                        <i />
                        {model.model}
                      </span>
                      <span>{formatTokens(model.totalTokens)}</span>
                      <span>{formatPercent(model.cacheHitRate)}</span>
                      <span>{formatInteger(model.calls)}</span>
                      <span>{formatCost(model.estimatedCostUsd)}</span>
                    </div>
                  ))
                )}
              </div>
            </article>

            <article className="panel project-panel">
              <PanelHeading code="B02" title="项目分布" meta="BY WORKSPACE" />
              <div className="model-table">
                <div className="table-row table-head">
                  <span>PROJECT</span>
                  <span>TOKENS</span>
                  <span>CALLS</span>
                </div>
                {snapshot.projects.length === 0 ? (
                  <EmptyLine />
                ) : (
                  snapshot.projects.slice(0, 8).map((project) => (
                    <div className="table-row" key={project.project}>
                      <span className="model-name">
                        <i />
                        {project.project}
                      </span>
                      <span>{formatTokens(project.totalTokens)}</span>
                      <span>{formatInteger(project.calls)}</span>
                    </div>
                  ))
                )}
              </div>
            </article>

            <article className="panel recent-panel">
              <PanelHeading code="B03" title="最近调用" meta="LOCAL ONLY" />
              <div className="recent-list">
                {snapshot.recent.length === 0 ? (
                  <EmptyLine />
                ) : (
                  <>
                    <div className="recent-table-head" aria-hidden="true">
                      <span>TIME</span>
                      <span>MODEL / THREAD</span>
                      <span>INPUT</span>
                      <span>CACHE</span>
                      <span>OUTPUT</span>
                      <span>COST</span>
                    </div>
                    {snapshot.recent.slice(0, 7).map((event) => (
                      <div className="recent-row" key={event.id}>
                        <time>{formatTime(event.occurredAt)}</time>
                        <div>
                          <strong>{event.model}</strong>
                          <span>{event.threadId.slice(0, 18)}</span>
                        </div>
                        <b>{formatTokens(event.freshInputTokens)}</b>
                        <b className="recent-cache">
                          {formatTokens(event.cachedInputTokens)}
                        </b>
                        <b>{formatTokens(event.outputTokens)}</b>
                        <b className="recent-cost">
                          {formatCost(event.estimatedCostUsd)}
                        </b>
                      </div>
                    ))}
                  </>
                )}
              </div>
            </article>
          </section>
        </>
      ) : null}
      {priceEditorOpen ? (
        <ModelPriceEditor onClose={() => setPriceEditorOpen(false)} />
      ) : null}
      {shareCardOpen && snapshot ? (
        <ShareCard
          snapshot={snapshot}
          preset={preset}
          onClose={() => setShareCardOpen(false)}
        />
      ) : null}
      {managingSources ? (
        <SourceManager
          homes={homes}
          onClose={() => setManagingSources(false)}
        />
      ) : null}
    </main>
  );
}

function SourceManager({
  homes,
  onClose,
}: {
  homes: Array<{ path: string; kind: SourceFilter; label: string }>;
  onClose: () => void;
}) {
  const [busyPath, setBusyPath] = useState<string>();

  async function addHome() {
    setBusyPath("__adding__");
    try {
      await chooseSourceHome();
    } catch (error) {
      window.alert(`添加数据目录失败：${String(error)}`);
    } finally {
      setBusyPath(undefined);
    }
  }

  async function removeHome(path: string) {
    if (
      !window.confirm(
        `移除数据目录 ${path}？\n该目录解析出的统计也会一并清除。`,
      )
    )
      return;
    setBusyPath(path);
    try {
      await removeSourceHome(path);
    } catch (error) {
      window.alert(`移除数据目录失败：${String(error)}`);
    } finally {
      setBusyPath(undefined);
    }
  }

  return (
    <div className="price-editor-overlay" role="presentation">
      <section
        className="price-editor source-manager"
        role="dialog"
        aria-modal="true"
        aria-labelledby="source-manager-title"
      >
        <header className="price-editor-heading">
          <div>
            <span>SOURCES</span>
            <h2 id="source-manager-title">数据目录</h2>
          </div>
          <button onClick={onClose} title="关闭">
            <X size={18} />
          </button>
        </header>
        <p className="price-editor-note">
          每个目录按其内部结构识别为 Claude Code、Codex 或
          ZCode，统计各自独立保存，可同时启用多个并在顶部切换查看。
        </p>
        <div className="price-editor-list">
          {homes.length === 0 ? (
            <div className="price-editor-empty">还没有数据目录</div>
          ) : (
            homes.map((home) => (
              <div className="price-editor-row" key={home.path}>
                <div className="price-model-name">
                  <strong>{home.label}</strong>
                  <small>{home.path}</small>
                </div>
                <button
                  className="price-save-button"
                  disabled={busyPath === home.path}
                  onClick={() => void removeHome(home.path)}
                >
                  {busyPath === home.path ? "移除中" : "移除"}
                </button>
              </div>
            ))
          )}
        </div>
        <footer className="price-editor-status">
          <button
            className="price-save-button"
            disabled={busyPath === "__adding__"}
            onClick={() => void addHome()}
          >
            <RotateCcw size={14} />
            {busyPath === "__adding__" ? "添加中" : "添加数据目录"}
          </button>
        </footer>
      </section>
    </div>
  );
}

function ModelPriceEditor({ onClose }: { onClose: () => void }) {
  const [prices, setPrices] = useState<ModelPriceEntry[]>([]);
  const [loading, setLoading] = useState(true);
  const [savingModel, setSavingModel] = useState<string>();
  const [message, setMessage] = useState<string>();

  useEffect(() => {
    let active = true;
    const loadPrices = () =>
      getUsedModelPrices()
        .then((entries) => {
          if (active) setPrices(entries);
        })
        .catch((error: unknown) => {
          if (active) setMessage(String(error));
        })
        .finally(() => {
          if (active) setLoading(false);
        });
    void loadPrices();
    void refreshModelsDevPrices()
      .then(() => loadPrices())
      .then(() => {
        if (active) setMessage("已同步 models.dev 最新价格");
      })
      .catch(() => {
        if (active) setMessage("models.dev 暂时不可用，当前显示上次缓存价格");
      });
    return () => {
      active = false;
    };
  }, []);

  function updatePrice(
    model: string,
    field:
      | "inputPerMillion"
      | "cachedInputPerMillion"
      | "outputPerMillion"
      | "multiplier",
    value: string,
  ) {
    setPrices((entries) =>
      entries.map((entry) =>
        entry.model === model ? { ...entry, [field]: value } : entry,
      ),
    );
    setMessage(undefined);
  }

  async function savePrice(entry: ModelPriceEntry) {
    setSavingModel(entry.model);
    setMessage(undefined);
    try {
      await updateModelPrice(entry);
      setPrices(await getUsedModelPrices());
      setMessage(`已保存 ${entry.model}`);
    } catch (error) {
      setMessage(String(error));
    } finally {
      setSavingModel(undefined);
    }
  }

  return (
    <div className="price-editor-overlay" role="presentation">
      <section
        className="price-editor"
        role="dialog"
        aria-modal="true"
        aria-labelledby="price-editor-title"
      >
        <header className="price-editor-heading">
          <div>
            <span>PRICING</span>
            <h2 id="price-editor-title">模型价格</h2>
          </div>
          <button onClick={onClose} title="关闭">
            <X size={18} />
          </button>
        </header>
        <p className="price-editor-note">
          可直接修改单价，单位为 USD / 1M Tokens；自定义价格不会被 models.dev
          刷新覆盖。倍率 `1` 为原价，`0.8` 表示八折。
        </p>
        <div className="price-editor-columns" aria-hidden="true">
          <span>模型</span>
          <span>INPUT</span>
          <span>CACHED INPUT</span>
          <span>OUTPUT</span>
          <span>倍率</span>
          <span />
        </div>
        <div className="price-editor-list">
          {loading ? (
            <div className="price-editor-empty">正在读取模型记录</div>
          ) : prices.length === 0 ? (
            <div className="price-editor-empty">
              当前目录还没有可定价的模型记录
            </div>
          ) : (
            prices.map((entry) => {
              const complete =
                isNonNegativeNumber(entry.inputPerMillion) &&
                isNonNegativeNumber(entry.cachedInputPerMillion) &&
                isNonNegativeNumber(entry.outputPerMillion) &&
                isNonNegativeNumber(entry.multiplier);
              return (
                <div className="price-editor-row" key={entry.model}>
                  <div className="price-model-name">
                    <strong>{entry.model}</strong>
                    <small>
                      {entry.customized
                        ? "用户自定义"
                        : entry.configured
                          ? "models.dev"
                          : "未匹配价格 · 可手动填写"}
                    </small>
                  </div>
                  <PriceInput
                    label={`${entry.model} input price`}
                    value={entry.inputPerMillion}
                    onChange={(value) =>
                      updatePrice(entry.model, "inputPerMillion", value)
                    }
                  />
                  <PriceInput
                    label={`${entry.model} cached input price`}
                    value={entry.cachedInputPerMillion}
                    onChange={(value) =>
                      updatePrice(entry.model, "cachedInputPerMillion", value)
                    }
                  />
                  <PriceInput
                    label={`${entry.model} output price`}
                    value={entry.outputPerMillion}
                    onChange={(value) =>
                      updatePrice(entry.model, "outputPerMillion", value)
                    }
                  />
                  <PriceInput
                    label={`${entry.model} price multiplier`}
                    value={entry.multiplier}
                    onChange={(value) =>
                      updatePrice(entry.model, "multiplier", value)
                    }
                  />
                  <button
                    className="price-save-button"
                    disabled={!complete || savingModel === entry.model}
                    onClick={() => void savePrice(entry)}
                  >
                    <Save size={15} />
                    {savingModel === entry.model ? "保存中" : "保存"}
                  </button>
                </div>
              );
            })
          )}
        </div>
        <footer className="price-editor-status">
          {message ?? "保存后历史费用会立即按新价格重新计算"}
        </footer>
      </section>
    </div>
  );
}

function isNonNegativeNumber(value: string) {
  if (value.trim() === "") return false;
  const number = Number(value);
  return Number.isFinite(number) && number >= 0;
}

function PriceInput({
  label,
  value,
  onChange,
}: {
  label: string;
  value: string;
  onChange: (value: string) => void;
}) {
  return (
    <input
      type="number"
      min="0"
      step="any"
      inputMode="decimal"
      aria-label={label}
      placeholder="0.00"
      value={value}
      onChange={(event) => onChange(event.target.value)}
    />
  );
}

function PanelHeading({
  code,
  title,
  meta,
}: {
  code: string;
  title: string;
  meta: string;
}) {
  return (
    <header className="panel-heading">
      <div>
        <span>{code}</span>
        <h2>{title}</h2>
      </div>
      <small>{meta}</small>
    </header>
  );
}

function TokenComposition({
  summary,
}: {
  summary: NonNullable<ReturnType<typeof useUsage>["data"]>["summary"];
}) {
  const total = Math.max(
    1,
    summary.freshInputTokens + summary.cachedInputTokens + summary.outputTokens,
  );
  const items = [
    {
      label: "缓存输入",
      value: summary.cachedInputTokens,
      className: "cached",
      icon: <Sparkles size={14} />,
    },
    {
      label: "新增输入",
      value: summary.freshInputTokens,
      className: "fresh",
      icon: <ArrowDownRight size={14} />,
    },
    {
      label: "模型输出",
      value: summary.outputTokens,
      className: "output",
      icon: <ArrowUpRight size={14} />,
    },
  ];
  return (
    <div className="composition">
      <div className="composition-bar">
        {items.map((item) => (
          <i
            key={item.label}
            className={item.className}
            style={{ width: `${(item.value / total) * 100}%` }}
          />
        ))}
      </div>
      <div className="composition-list">
        {items.map((item) => (
          <div key={item.label}>
            <span className={item.className}>{item.icon}</span>
            <p>
              <small>{item.label}</small>
              <strong>{formatTokens(item.value)}</strong>
            </p>
            <b>{Math.round((item.value / total) * 100)}%</b>
          </div>
        ))}
      </div>
    </div>
  );
}

function normalizeTrendCosts(trends: UsageTrendPoint[]): TrendChartPoint[] {
  return trends.map((point) => {
    const costUsd =
      point.estimatedCostUsd === null ? null : Number(point.estimatedCostUsd);
    return {
      ...point,
      costUsd: Number.isFinite(costUsd) ? costUsd : null,
    };
  });
}

function TrendTooltip({
  active,
  payload,
}: {
  active?: boolean;
  payload?: Array<{ payload: TrendChartPoint }>;
}) {
  const point = payload?.[0]?.payload;
  if (!active || !point) return null;
  return (
    <div className="chart-tooltip">
      <time>{formatTime(point.bucketStart, true)}</time>
      <strong>{formatTokens(point.totalTokens)} tokens</strong>
      <span>
        缓存 {formatTokens(point.cachedInputTokens)} · 输出{" "}
        {formatTokens(point.outputTokens)}
      </span>
      <b>
        {point.costUsd !== null && point.hasUnpricedUsage
          ? "本桶已定价 COST "
          : "本桶总 COST "}
        {formatCost(point.costUsd === null ? null : String(point.costUsd), 1)}
      </b>
    </div>
  );
}

function LoadingState() {
  return (
    <section className="loading-state">
      <span />
      <p>正在读取本机 AI 编码记录</p>
      <small>首次扫描可能需要一点时间</small>
    </section>
  );
}

function ErrorState({ onRetry }: { onRetry: () => Promise<void> }) {
  return (
    <section className="error-state">
      <Braces size={26} />
      <h2>无法读取统计数据</h2>
      <p>请确认 `~/.codex/sessions` 存在且当前用户可读取。</p>
      <button onClick={() => void onRetry()}>重新扫描</button>
    </section>
  );
}

function EmptyLine() {
  return (
    <div className="empty-line">当前时间范围没有可用的 token_count 事件</div>
  );
}
