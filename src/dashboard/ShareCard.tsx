import { useEffect, useRef } from "react";
import { Download, X } from "lucide-react";
import { formatTokens } from "../lib/format";
import type { RangePreset, UsageSnapshot } from "../types";

const CARD_W = 1080;
const CARD_H = 608;

const rangeTitle: Record<RangePreset, string> = {
  today: "今日",
  "24h": "过去 24 小时",
  "7d": "过去 7 天",
  "30d": "过去 30 天",
};

export function ShareCard({
  snapshot,
  preset,
  onClose,
}: {
  snapshot: UsageSnapshot;
  preset: RangePreset;
  onClose: () => void;
}) {
  const canvasRef = useRef<HTMLCanvasElement>(null);

  const stats = (() => {
    const topModel = snapshot.models[0];
    const topProject = snapshot.projects[0];
    const savedByCache = Math.round(
      snapshot.summary.cachedInputTokens * (1 - 0.15 / 1.75),
    );
    return { topModel, topProject, savedByCache };
  })();

  function render() {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;

    const gradient = ctx.createLinearGradient(0, 0, CARD_W, CARD_H);
    gradient.addColorStop(0, "#0d1b3e");
    gradient.addColorStop(0.55, "#17255a");
    gradient.addColorStop(1, "#3b1f5e");
    ctx.fillStyle = gradient;
    ctx.fillRect(0, 0, CARD_W, CARD_H);

    ctx.fillStyle = "rgba(255,255,255,.06)";
    for (let i = 0; i < 12; i += 1) {
      ctx.fillRect(60 + i * 84, 0, 1, CARD_H);
    }

    ctx.fillStyle = "#9db1e8";
    ctx.font = "600 22px Consolas, monospace";
    ctx.fillText("LOCAL TELEMETRY WRAPPED", 64, 84);
    ctx.fillStyle = "#ffffff";
    ctx.font = "700 30px 'Microsoft YaHei UI', sans-serif";
    ctx.fillText(`${snapshot.sourceLabel} · ${rangeTitle[preset]}`, 64, 130);

    ctx.fillStyle = "#ffffff";
    ctx.font = "700 104px 'Microsoft YaHei UI', sans-serif";
    ctx.fillText(formatTokens(snapshot.summary.totalTokens), 60, 268);
    ctx.fillStyle = "#9db1e8";
    ctx.font = "600 24px Consolas, monospace";
    ctx.fillText("TOKENS PROCESSED", 64, 308);
    ctx.fillStyle = "#8bc6ad";
    ctx.fillText(
      `${snapshot.summary.calls} CALLS · ${snapshot.summary.threads} THREADS`,
      64,
      348,
    );

    const columns = [
      {
        label: "TOP MODEL",
        value: stats.topModel?.model ?? "—",
        detail: stats.topModel
          ? `${formatTokens(stats.topModel.totalTokens)} tokens`
          : "暂无记录",
      },
      {
        label: "TOP PROJECT",
        value: stats.topProject?.project ?? "—",
        detail: stats.topProject
          ? `${formatTokens(stats.topProject.totalTokens)} tokens`
          : "暂无记录",
      },
      {
        label: "CACHE HIT",
        value: `${Math.round(snapshot.summary.cacheHitRate * 100)}%`,
        detail: `缓存输入 ${formatTokens(snapshot.summary.cachedInputTokens)}`,
      },
    ];
    columns.forEach((column, index) => {
      const x = 64 + index * 330;
      ctx.fillStyle = "rgba(255,255,255,.08)";
      ctx.fillRect(x, 396, 300, 140);
      ctx.fillStyle = "#9db1e8";
      ctx.font = "600 18px Consolas, monospace";
      ctx.fillText(column.label, x + 22, 436);
      ctx.fillStyle = "#ffffff";
      ctx.font = "700 30px 'Microsoft YaHei UI', sans-serif";
      ctx.fillText(
        column.value.length > 22
          ? `${column.value.slice(0, 21)}…`
          : column.value,
        x + 22,
        482,
      );
      ctx.fillStyle = "#9db1e8";
      ctx.font = "500 18px 'Microsoft YaHei UI', sans-serif";
      ctx.fillText(column.detail, x + 22, 514);
    });

    ctx.fillStyle = "rgba(255,255,255,.55)";
    ctx.font = "500 18px 'Microsoft YaHei UI', sans-serif";
    ctx.fillText(
      `QuotaLoom · 本地统计 · 生成于 ${new Date().toLocaleDateString("zh-CN")}`,
      64,
      576,
    );
  }

  useEffect(() => {
    render();
    // 卡片数据来自打开时的快照，只需绘制一次
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  function download() {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const link = document.createElement("a");
    link.download = `quota-loom-wrapped-${Date.now()}.png`;
    link.href = canvas.toDataURL("image/png");
    link.click();
  }

  return (
    <div className="price-editor-overlay" role="presentation">
      <section
        className="price-editor share-card"
        role="dialog"
        aria-modal="true"
        aria-labelledby="share-card-title"
      >
        <header className="price-editor-heading">
          <div>
            <span>SHARE</span>
            <h2 id="share-card-title">用量分享卡</h2>
          </div>
          <button onClick={onClose} title="关闭">
            <X size={18} />
          </button>
        </header>
        <p className="price-editor-note">
          卡片在本地渲染，只包含汇总数字，不含会话内容或路径。
        </p>
        <canvas
          ref={canvasRef}
          width={CARD_W}
          height={CARD_H}
          style={{ width: "100%", borderRadius: 12 }}
        />
        <footer className="price-editor-status">
          <button className="price-save-button" onClick={download}>
            <Download size={14} />
            保存 PNG
          </button>
        </footer>
      </section>
    </div>
  );
}
