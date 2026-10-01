import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import type {
  ModelPriceEntry,
  SourceHome,
  SyncResult,
  UsageRange,
  UsageSnapshot,
  WeeklyUsage,
} from "../types";

export function isDesktopRuntime() {
  return "__TAURI_INTERNALS__" in window;
}

export async function getUsageSnapshot(
  range: UsageRange,
  source: string = "all",
): Promise<UsageSnapshot> {
  if (!isDesktopRuntime()) {
    return demoSnapshot(range, source);
  }
  return invoke<UsageSnapshot>("get_usage_snapshot", { range, source });
}

export async function getSourceHomes(): Promise<SourceHome[]> {
  if (!isDesktopRuntime()) return [];
  return invoke<SourceHome[]>("get_source_homes");
}

export async function chooseSourceHome(
  currentPath?: string,
): Promise<SourceHome | null> {
  if (!isDesktopRuntime()) return null;
  const selected = await open({
    directory: true,
    multiple: false,
    defaultPath: currentPath,
    title: "添加 Claude Code / Codex / ZCode 数据目录",
  });
  if (typeof selected !== "string") return null;
  return invoke<SourceHome>("add_source_home", { path: selected });
}

export async function removeSourceHome(path: string): Promise<void> {
  if (!isDesktopRuntime()) return;
  await invoke("remove_source_home", { path });
}

export async function getAccountWeeklyUsage(): Promise<WeeklyUsage | null> {
  if (!isDesktopRuntime()) {
    return {
      usedPercent: 32,
      remainingPercent: 68,
      resetsAt: Math.floor(Date.now() / 1000) + 3 * 86_400,
    };
  }
  return invoke<WeeklyUsage | null>("get_account_weekly_usage");
}

export async function syncUsage(): Promise<SyncResult> {
  if (!isDesktopRuntime()) {
    return {
      scannedFiles: 18,
      changedFiles: 1,
      importedEvents: 3,
      rebuiltFiles: 0,
      warnings: [],
    };
  }
  return invoke<SyncResult>("sync_usage");
}

export async function resetUsageCache(): Promise<SyncResult> {
  if (!isDesktopRuntime()) {
    return {
      scannedFiles: 18,
      changedFiles: 18,
      importedEvents: 240,
      rebuiltFiles: 0,
      warnings: [],
    };
  }
  return invoke<SyncResult>("reset_usage_cache");
}

export async function showDashboardWindow() {
  if (isDesktopRuntime()) await invoke("show_dashboard_window");
}

export async function showFloatingWindow() {
  if (isDesktopRuntime()) await invoke("show_floating_window");
}

export async function hideFloatingWindow() {
  if (isDesktopRuntime()) await invoke("hide_floating_window");
}

export async function getUsedModelPrices() {
  if (!isDesktopRuntime()) {
    return [
      {
        model: "gpt-5.3-codex",
        inputPerMillion: "1.75",
        cachedInputPerMillion: "0.175",
        outputPerMillion: "14",
        multiplier: "1",
        configured: true,
        customized: false,
      },
      {
        model: "custom-codex",
        inputPerMillion: "",
        cachedInputPerMillion: "",
        outputPerMillion: "",
        multiplier: "1",
        configured: false,
        customized: false,
      },
    ] satisfies ModelPriceEntry[];
  }
  return invoke<ModelPriceEntry[]>("get_used_model_prices");
}

export async function refreshModelsDevPrices() {
  if (!isDesktopRuntime()) return 2;
  return invoke<number>("refresh_models_dev_prices");
}

export async function updateModelPrice(entry: ModelPriceEntry) {
  if (isDesktopRuntime())
    await invoke("update_model_price", {
      model: entry.model,
      inputPerMillion: entry.inputPerMillion,
      cachedInputPerMillion: entry.cachedInputPerMillion,
      outputPerMillion: entry.outputPerMillion,
      multiplier: entry.multiplier,
    });
}

function demoSnapshot(range: UsageRange, source = "all"): UsageSnapshot {
  const labels: Record<string, { kind: string; label: string; brand: string }> =
    {
      all: { kind: "all", label: "全部来源", brand: "ALL SOURCES / USAGE" },
      claudeCode: {
        kind: "claudeCode",
        label: "Claude Code",
        brand: "CLAUDE CODE / USAGE",
      },
      codexCli: {
        kind: "codexCli",
        label: "Codex CLI",
        brand: "CODEX CLI / USAGE",
      },
      chatGptCodex: {
        kind: "chatGptCodex",
        label: "ChatGPT Codex",
        brand: "CHATGPT CODEX / USAGE",
      },
      zcode: { kind: "zcode", label: "ZCode", brand: "ZCODE / USAGE" },
    };
  const identity = labels[source] ?? labels.all;
  const models = [
    {
      model: "gpt-5.3-codex",
      totalTokens: 1_068_420,
      freshInputTokens: 238_440,
      cachedInputTokens: 612_800,
      outputTokens: 217_180,
      calls: 29,
      cacheHitRate: 0.72,
      estimatedCostUsd: "4.2718",
    },
    {
      model: "gpt-5.2-codex",
      totalTokens: 194_810,
      freshInputTokens: 54_200,
      cachedInputTokens: 108_940,
      outputTokens: 31_670,
      calls: 8,
      cacheHitRate: 0.668,
      estimatedCostUsd: "0.5571",
    },
  ];
  const span = Math.max(range.endAt - range.startAt, range.bucketSeconds * 8);
  const points = Math.min(
    32,
    Math.max(8, Math.ceil(span / range.bucketSeconds)),
  );
  const trends = Array.from({ length: points }, (_, index) => {
    const pulse =
      0.52 + Math.sin(index * 1.4) * 0.24 + Math.cos(index * 0.47) * 0.18;
    const fresh = Math.max(0, Math.round(8_000 + pulse * 14_000));
    const cached = Math.max(0, Math.round(18_000 + pulse * 38_000));
    const output = Math.max(0, Math.round(5_000 + pulse * 11_000));
    return {
      bucketStart: range.endAt - (points - 1 - index) * range.bucketSeconds,
      totalTokens: fresh + cached + output,
      freshInputTokens: fresh,
      cachedInputTokens: cached,
      outputTokens: output,
      calls: Math.max(1, Math.round(2 + pulse * 3)),
      estimatedCostUsd: (
        (fresh * 1.75 + cached * 0.175 + output * 14) /
        1_000_000
      ).toFixed(6),
      hasUnpricedUsage: false,
    };
  });
  return {
    generatedAt: Math.floor(Date.now() / 1000),
    codexHome: "~/.codex",
    sourceKind: identity.kind as UsageSnapshot["sourceKind"],
    sourceLabel: identity.label,
    sourceBrand: identity.brand,
    summary: {
      totalTokens: models.reduce((sum, model) => sum + model.totalTokens, 0),
      freshInputTokens: models.reduce(
        (sum, model) => sum + model.freshInputTokens,
        0,
      ),
      cachedInputTokens: models.reduce(
        (sum, model) => sum + model.cachedInputTokens,
        0,
      ),
      outputTokens: models.reduce((sum, model) => sum + model.outputTokens, 0),
      calls: 37,
      threads: 9,
      cacheHitRate: 0.711,
      estimatedCostUsd: "4.8289",
      unpricedModels: 0,
    },
    trends,
    models,
    projects: [
      { project: "demo-project", totalTokens: 628_400, calls: 21 },
      { project: "side-quest", totalTokens: 171_300, calls: 8 },
    ],
    recent: models.flatMap((model, modelIndex) =>
      Array.from({ length: modelIndex === 0 ? 5 : 2 }, (_, index) => ({
        id: `${model.model}-${index}`,
        threadId: `thread-${modelIndex + 1}-${index + 1}`,
        occurredAt: Math.floor(Date.now() / 1000) - (index + modelIndex) * 860,
        model: model.model,
        totalTokens: 28_000 + index * 7_420,
        freshInputTokens: 5_900 + index * 1_100,
        cachedInputTokens: 17_300 + index * 5_700,
        outputTokens: 4_800 + index * 620,
        estimatedCostUsd: (0.15 + index * 0.03).toFixed(4),
      })),
    ),
    quotaEstimate: null,
  };
}
