export interface UsageRange {
  startAt: number;
  endAt: number;
  bucketSeconds: number;
  timezoneOffsetSeconds: number;
}

export interface UsageSummary {
  totalTokens: number;
  freshInputTokens: number;
  cachedInputTokens: number;
  outputTokens: number;
  calls: number;
  threads: number;
  cacheHitRate: number;
  estimatedCostUsd: string | null;
  unpricedModels: number;
}

export interface UsageTrendPoint {
  bucketStart: number;
  totalTokens: number;
  freshInputTokens: number;
  cachedInputTokens: number;
  outputTokens: number;
  calls: number;
  estimatedCostUsd: string | null;
  hasUnpricedUsage: boolean;
}

export interface ModelUsage {
  model: string;
  totalTokens: number;
  freshInputTokens: number;
  cachedInputTokens: number;
  outputTokens: number;
  calls: number;
  cacheHitRate: number;
  estimatedCostUsd: string | null;
}

export interface ModelPriceEntry {
  model: string;
  inputPerMillion: string;
  cachedInputPerMillion: string;
  outputPerMillion: string;
  multiplier: string;
  configured: boolean;
  customized: boolean;
}

export interface ProjectUsage {
  project: string;
  totalTokens: number;
  calls: number;
}

export interface RecentUsageEvent {
  id: string;
  threadId: string;
  occurredAt: number;
  model: string;
  totalTokens: number;
  freshInputTokens: number;
  cachedInputTokens: number;
  outputTokens: number;
  estimatedCostUsd: string | null;
}

export type SourceFilter =
  "all" | "claudeCode" | "codexCli" | "chatGptCodex" | "zcode";

export interface SourceHome {
  path: string;
  kind: SourceFilter;
  label: string;
}

export interface UsageSnapshot {
  generatedAt: number;
  codexHome: string;
  sourceKind: "all" | "claudeCode" | "codexCli" | "chatGptCodex" | "zcode";
  sourceLabel: string;
  sourceBrand: string;
  summary: UsageSummary;
  trends: UsageTrendPoint[];
  models: ModelUsage[];
  projects: ProjectUsage[];
  recent: RecentUsageEvent[];
  quotaEstimate?: QuotaEstimate | null;
}

export interface WeeklyUsage {
  usedPercent: number;
  remainingPercent: number;
  resetsAt: number | null;
}

export interface QuotaEstimate {
  planName: string;
  tokensPerDay: number;
  usedTokens: number;
  usedPercent: number;
  remainingPercent: number;
  resetsAt: number;
}

export interface SyncResult {
  scannedFiles: number;
  changedFiles: number;
  importedEvents: number;
  rebuiltFiles: number;
  warnings: string[];
}

export type RangePreset = "today" | "24h" | "7d" | "30d";
