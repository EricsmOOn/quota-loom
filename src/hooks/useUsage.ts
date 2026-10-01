import { useEffect } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  keepPreviousData,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import {
  getAccountWeeklyUsage,
  getUsageSnapshot,
  getSourceHomes,
  isDesktopRuntime,
} from "../lib/api";
import { resolveRange } from "../lib/range";
import type { RangePreset, SourceFilter } from "../types";

const SOURCE_FILTER_KEY = "quota-loom-source-filter";

export function loadSourceFilter(): SourceFilter {
  const stored = localStorage.getItem(SOURCE_FILTER_KEY);
  if (
    stored === "all" ||
    stored === "claudeCode" ||
    stored === "codexCli" ||
    stored === "chatGptCodex" ||
    stored === "zcode"
  ) {
    return stored;
  }
  return "all";
}

export function saveSourceFilter(filter: SourceFilter) {
  localStorage.setItem(SOURCE_FILTER_KEY, filter);
}

export function useUsage(preset: RangePreset, source: SourceFilter = "all") {
  return useQuery({
    queryKey: ["usage", preset, source],
    queryFn: () => getUsageSnapshot(resolveRange(preset), source),
    placeholderData: keepPreviousData,
    refetchInterval: 15_000,
    refetchIntervalInBackground: true,
  });
}

export function useSourceHomes() {
  return useQuery({
    queryKey: ["source-homes"],
    queryFn: getSourceHomes,
    staleTime: 60_000,
  });
}

export function useWeeklyUsage() {
  return useQuery({
    queryKey: ["account-weekly-usage"],
    queryFn: getAccountWeeklyUsage,
    refetchInterval: 60_000,
    refetchIntervalInBackground: true,
    retry: false,
    staleTime: 45_000,
  });
}

export function useUsageEvents() {
  const client = useQueryClient();
  useEffect(() => {
    if (!isDesktopRuntime()) return;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void listen("usage-updated", () => {
      void client.invalidateQueries({ queryKey: ["usage"] });
      void client.invalidateQueries({ queryKey: ["source-homes"] });
    }).then((off) => {
      if (disposed) off();
      else unlisten = off;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [client]);
}
