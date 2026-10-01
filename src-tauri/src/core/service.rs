use crate::core::database::UsageDatabase;
use crate::core::parser::{parse_session_file_for_source, source_key, ZCODE_DB_FILE_NAME};
use crate::core::pricing::CatalogModelPrice;
use crate::core::types::{
    DataSourceKind, ModelPriceEntry, QuotaEstimate, SyncResult, UsageRange, UsageSnapshot,
    WeeklyUsage,
};
use directories::{ProjectDirs, UserDirs};
use notify::{PollWatcher, RecommendedWatcher, RecursiveMode, Watcher};
use serde::Deserialize;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};
use walkdir::WalkDir;

const WEEKLY_WINDOW_MINUTES: i64 = 7 * 24 * 60;
const WEEKLY_USAGE_CACHE_TTL: Duration = Duration::from_secs(60);
const APP_SERVER_TIMEOUT: Duration = Duration::from_secs(8);

const MODELS_DEV_PROVIDERS: &[&str] = &[
    "openai",
    "anthropic",
    "deepseek",
    "google",
    "google-vertex",
    "google-vertex-anthropic",
    "mistral",
    "cohere",
    "xai",
    "groq",
    "meta",
    "llama",
    "alibaba",
    "alibaba-cn",
    "kimi-for-coding",
    "moonshotai",
    "moonshotai-cn",
    "minimax",
    "minimax-cn",
    "zai",
    "zai-coding-plan",
    "perplexity",
    "nvidia",
    "togetherai",
    "fireworks-ai",
    "amazon-bedrock",
    "azure",
];

pub struct UsageService {
    homes: RwLock<Vec<PathBuf>>,
    database: UsageDatabase,
    operation_lock: Mutex<()>,
    watch_sender: Mutex<Option<Sender<WatchSignal>>>,
    weekly_usage_cache: Mutex<Option<CachedWeeklyUsage>>,
}

struct CachedWeeklyUsage {
    data_home: PathBuf,
    fetched_at: Instant,
    value: Option<WeeklyUsage>,
}

#[derive(Clone, Copy)]
enum WatchSignal {
    FilesChanged,
    Reconfigure,
}

impl UsageService {
    pub fn open_default() -> Result<Self, String> {
        let project_dirs = app_project_dirs()?;
        let database_path = project_dirs.data_local_dir().join("usage.sqlite3");
        Self::open_homes(load_source_homes()?, database_path)
    }

    /// 单 Home 入口（测试与兼容使用）。
    pub fn open(data_home: PathBuf, database_path: PathBuf) -> Result<Self, String> {
        Self::open_homes(vec![data_home], database_path)
    }

    pub fn open_homes(homes: Vec<PathBuf>, database_path: PathBuf) -> Result<Self, String> {
        Ok(Self {
            homes: RwLock::new(homes),
            database: UsageDatabase::open(&database_path)?,
            operation_lock: Mutex::new(()),
            watch_sender: Mutex::new(None),
            weekly_usage_cache: Mutex::new(None),
        })
    }

    pub fn source_homes(&self) -> Vec<PathBuf> {
        self.homes
            .read()
            .map(|homes| homes.clone())
            .unwrap_or_default()
    }

    /// 主 Home：快照展示与周额度探测使用。
    fn primary_home(&self) -> Option<PathBuf> {
        let homes = self.source_homes();
        homes
            .iter()
            .find(|home| {
                matches!(
                    detect_data_source(home),
                    DataSourceKind::CodexCli | DataSourceKind::ChatGptCodex
                )
            })
            .or_else(|| homes.first())
            .cloned()
    }

    pub fn primary_source_kind(&self) -> DataSourceKind {
        self.primary_home()
            .map(|home| detect_data_source(&home))
            .unwrap_or(DataSourceKind::All)
    }

    pub fn add_source_home(&self, path: PathBuf) -> Result<DataSourceKind, String> {
        if !path.is_dir() {
            return Err(format!("所选目录不存在: {}", path.display()));
        }
        {
            let _operation = self
                .operation_lock
                .lock()
                .map_err(|error| error.to_string())?;
            let mut homes = self.source_homes();
            if homes.iter().any(|home| home == &path) {
                return Ok(detect_data_source(&path));
            }
            homes.push(path.clone());
            save_source_homes(&homes)?;
            *self.homes.write().map_err(|error| error.to_string())? = homes;
        }
        if let Some(sender) = self
            .watch_sender
            .lock()
            .map_err(|error| error.to_string())?
            .as_ref()
        {
            let _ = sender.send(WatchSignal::Reconfigure);
        }
        let _ = self.sync();
        Ok(detect_data_source(&path))
    }

    pub fn remove_source_home(&self, path: &Path) -> Result<(), String> {
        {
            let _operation = self
                .operation_lock
                .lock()
                .map_err(|error| error.to_string())?;
            let mut homes = self.source_homes();
            homes.retain(|home| home != path);
            if homes.is_empty() {
                return Err("至少保留一个数据目录".to_string());
            }
            save_source_homes(&homes)?;
            *self.homes.write().map_err(|error| error.to_string())? = homes;
            self.database.purge_source_home(path)?;
        }
        if let Some(sender) = self
            .watch_sender
            .lock()
            .map_err(|error| error.to_string())?
            .as_ref()
        {
            let _ = sender.send(WatchSignal::Reconfigure);
        }
        Ok(())
    }

    pub fn sync(&self) -> Result<SyncResult, String> {
        let _operation = self
            .operation_lock
            .lock()
            .map_err(|error| error.to_string())?;
        let mut result = SyncResult::default();
        for home in self.source_homes() {
            let source_kind = detect_data_source(&home);
            for path in collect_session_files(&home, source_kind) {
                result.scanned_files += 1;
                let key = source_key(&path);
                let cursor = match self.database.load_cursor(&key) {
                    Ok(cursor) => cursor,
                    Err(error) => {
                        result
                            .warnings
                            .push(format!("读取游标失败 {}: {error}", path.display()));
                        continue;
                    }
                };
                let previous_cursor = cursor.clone();
                if source_kind == DataSourceKind::ZCode
                    && path.file_name().and_then(|name| name.to_str()) == Some(ZCODE_DB_FILE_NAME)
                    && previous_cursor.is_none()
                {
                    // ZCode 数据库源首次启用：清理由 rollout jsonl 解析出的旧统计，避免双计
                    if let Err(error) = self.database.purge_legacy_zcode_rollout_events() {
                        result
                            .warnings
                            .push(format!("清理旧版 ZCode 统计失败: {error}"));
                    }
                }
                let outcome = match parse_session_file_for_source(&path, cursor, source_kind) {
                    Ok(outcome) => outcome,
                    Err(error) => {
                        result.warnings.push(error);
                        continue;
                    }
                };
                if outcome.reset_required {
                    result.rebuilt_files += 1;
                }
                if outcome.events.is_empty()
                    && !outcome.reset_required
                    && previous_cursor.as_ref() == Some(&outcome.cursor)
                {
                    continue;
                }
                result.changed_files += 1;
                match self.database.apply_parse_outcome(&outcome, source_kind) {
                    Ok(inserted) => result.imported_events += inserted,
                    Err(error) => result
                        .warnings
                        .push(format!("写入统计失败 {}: {error}", path.display())),
                }
            }
        }
        Ok(result)
    }

    pub fn reset_usage_cache(&self) -> Result<SyncResult, String> {
        let operation = self
            .operation_lock
            .lock()
            .map_err(|error| error.to_string())?;
        self.database.clear_usage()?;
        drop(operation);
        self.sync()
    }

    pub fn snapshot(
        &self,
        range: UsageRange,
        filter: DataSourceKind,
    ) -> Result<UsageSnapshot, String> {
        let display_home = self.primary_home().unwrap_or_default();
        let (display_kind, source_filter) = if filter == DataSourceKind::All {
            (DataSourceKind::All, None)
        } else {
            (filter, Some(filter.id_key()))
        };
        let mut snapshot =
            self.database
                .snapshot(&display_home, display_kind, range, source_filter)?;
        if filter == DataSourceKind::ZCode {
            snapshot.quota_estimate = self.zcode_quota_estimate();
        }
        Ok(snapshot)
    }

    /// ZCode 套餐余额没有本地缓存可读，按平台计费口径（input + output）
    /// 对配置的每日额度做估算；未配置（或额度为 0）时返回 None，UI 不渲染。
    fn zcode_quota_estimate(&self) -> Option<QuotaEstimate> {
        let preference = load_zcode_quota_preference()?;
        if preference.tokens_per_day == 0 {
            return None;
        }
        let now_local = chrono::Local::now();
        let today_start = now_local
            .date_naive()
            .and_hms_opt(0, 0, 0)
            .and_then(|time| time.and_local_timezone(chrono::Local).earliest())
            .map(|time| time.timestamp())?;
        let used_tokens = self
            .database
            .query_platform_tokens(today_start, now_local.timestamp())
            .ok()?;
        Some(build_quota_estimate(&preference, used_tokens, now_local))
    }

    pub fn account_weekly_usage(&self) -> Option<WeeklyUsage> {
        // 周额度是 Codex 家族的能力：探测第一个 Codex 家族 Home
        let data_home = self.source_homes().into_iter().find(|home| {
            matches!(
                detect_data_source(home),
                DataSourceKind::CodexCli | DataSourceKind::ChatGptCodex
            )
        })?;
        let mut cache = self.weekly_usage_cache.lock().ok()?;
        if let Some(cached) = cache.as_ref() {
            if cached.data_home == data_home && cached.fetched_at.elapsed() < WEEKLY_USAGE_CACHE_TTL
            {
                return cached.value.clone();
            }
        }

        let value = query_account_weekly_usage(&data_home);
        *cache = Some(CachedWeeklyUsage {
            data_home,
            fetched_at: Instant::now(),
            value: value.clone(),
        });
        value
    }

    pub fn used_model_prices(&self) -> Result<Vec<ModelPriceEntry>, String> {
        self.database.used_model_prices()
    }

    pub fn update_model_price(
        &self,
        model: &str,
        input_per_million: &str,
        cached_input_per_million: &str,
        output_per_million: &str,
        multiplier: &str,
    ) -> Result<(), String> {
        self.database.update_model_price(
            model,
            input_per_million,
            cached_input_per_million,
            output_per_million,
            multiplier,
        )
    }

    pub fn refresh_models_dev_prices(&self) -> Result<u64, String> {
        let response = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(12))
            .build()
            .map_err(|error| error.to_string())?
            .get("https://models.dev/api.json")
            .send()
            .and_then(reqwest::blocking::Response::error_for_status)
            .map_err(|error| format!("无法读取 models.dev: {error}"))?
            .json::<serde_json::Value>()
            .map_err(|error| format!("无法解析 models.dev: {error}"))?;
        let mut prices = Vec::new();
        for provider in MODELS_DEV_PROVIDERS {
            let Some(models) = response
                .get(provider)
                .and_then(|value| value.get("models"))
                .and_then(serde_json::Value::as_object)
            else {
                continue;
            };
            for (model, value) in models {
                let Some(input) = catalog_number(value.pointer("/cost/input")) else {
                    continue;
                };
                let Some(output) = catalog_number(value.pointer("/cost/output")) else {
                    continue;
                };
                let cached = catalog_number(value.pointer("/cost/cache_read"))
                    .unwrap_or_else(|| input.clone());
                let display_name = value
                    .get("name")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or(model)
                    .to_string();
                prices.push(CatalogModelPrice {
                    model: model.to_string(),
                    display_name,
                    input_per_million: input,
                    cached_input_per_million: cached,
                    output_per_million: output,
                });
            }
        }
        if prices.is_empty() {
            return Err("models.dev 未返回可用模型目录".to_string());
        }
        self.database.import_catalog_prices(&prices)
    }

    pub fn start_background_sync(
        self: &Arc<Self>,
        on_update: impl Fn() + Send + Sync + 'static,
    ) -> Result<(), String> {
        let service = Arc::clone(self);
        let callback = Arc::new(on_update);
        let (sender, receiver) = std::sync::mpsc::channel::<WatchSignal>();
        let event_sender = sender.clone();
        let mut watcher = create_multi_home_watcher(&service.source_homes(), event_sender.clone())?;
        *self
            .watch_sender
            .lock()
            .map_err(|error| error.to_string())? = Some(sender);
        std::thread::spawn(move || {
            while let Ok(signal) = receiver.recv() {
                let mut files_changed = matches!(signal, WatchSignal::FilesChanged);
                let mut reconfigure = matches!(signal, WatchSignal::Reconfigure);
                if files_changed {
                    std::thread::sleep(Duration::from_millis(300));
                }
                while let Ok(signal) = receiver.try_recv() {
                    files_changed |= matches!(signal, WatchSignal::FilesChanged);
                    reconfigure |= matches!(signal, WatchSignal::Reconfigure);
                }
                if reconfigure {
                    if let Ok(replacement) =
                        create_multi_home_watcher(&service.source_homes(), event_sender.clone())
                    {
                        watcher = replacement;
                    }
                }
                if files_changed {
                    if let Ok(result) = service.sync() {
                        if result.imported_events > 0 || result.rebuilt_files > 0 {
                            callback();
                        }
                    }
                }
                let _watcher_guard = &watcher;
            }
        });
        Ok(())
    }
}

/// 为所有数据 Home 建一个共享 watcher（notify 支持单 watcher 多路径）。
fn create_multi_home_watcher(
    homes: &[PathBuf],
    sender: Sender<WatchSignal>,
) -> Result<Box<dyn Watcher + Send>, String> {
    let handler = move |event: notify::Result<notify::Event>| {
        if event.is_ok() {
            let _ = sender.send(WatchSignal::FilesChanged);
        }
    };
    let polling = homes.iter().any(|home| requires_polling_watcher(home));
    let mut watcher: Box<dyn Watcher + Send> = if polling {
        Box::new(
            PollWatcher::new(
                handler,
                notify::Config::default()
                    .with_poll_interval(Duration::from_secs(1))
                    .with_compare_contents(false),
            )
            .map_err(|error| error.to_string())?,
        )
    } else {
        Box::new(
            RecommendedWatcher::new(handler, notify::Config::default())
                .map_err(|error| error.to_string())?,
        )
    };
    for home in homes {
        if home.exists() {
            let _ = watcher.watch(home, RecursiveMode::Recursive);
        }
    }
    Ok(watcher)
}

fn query_account_weekly_usage(data_home: &Path) -> Option<WeeklyUsage> {
    let mut child = start_codex_app_server(data_home)?;
    let (Some(mut stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
        terminate_app_server(&mut child);
        return None;
    };
    let requests = [
        json!({
            "method": "initialize",
            "id": 1,
            "params": {
                "clientInfo": {
                    "name": "quota_loom",
                    "title": "QuotaLoom",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }
        }),
        json!({ "method": "initialized" }),
        json!({
            "method": "account/read",
            "id": 2,
            "params": { "refreshToken": false }
        }),
        json!({ "method": "account/rateLimits/read", "id": 3 }),
    ];
    for request in requests {
        if serde_json::to_writer(&mut stdin, &request).is_err() || stdin.write_all(b"\n").is_err() {
            drop(stdin);
            terminate_app_server(&mut child);
            return None;
        }
    }
    if stdin.flush().is_err() {
        drop(stdin);
        terminate_app_server(&mut child);
        return None;
    }

    let (sender, receiver) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            let Ok(message) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if matches!(message.get("id").and_then(Value::as_i64), Some(2 | 3))
                && sender.send(message).is_err()
            {
                break;
            }
        }
    });

    let deadline = Instant::now() + APP_SERVER_TIMEOUT;
    let mut chatgpt_account = None;
    let mut rate_limits = None;
    let mut rate_limits_received = false;
    while Instant::now() < deadline
        && chatgpt_account != Some(false)
        && (chatgpt_account.is_none() || !rate_limits_received)
    {
        let timeout = deadline.saturating_duration_since(Instant::now());
        let Ok(message) = receiver.recv_timeout(timeout) else {
            break;
        };
        match message.get("id").and_then(Value::as_i64) {
            Some(2) => {
                chatgpt_account = Some(
                    message
                        .pointer("/result/account/type")
                        .and_then(Value::as_str)
                        == Some("chatgpt"),
                );
            }
            Some(3) => {
                rate_limits_received = true;
                rate_limits = message.get("result").cloned();
            }
            _ => {}
        }
    }

    drop(stdin);
    terminate_app_server(&mut child);
    let _ = reader.join();

    if chatgpt_account != Some(true) {
        return None;
    }
    parse_weekly_usage(&rate_limits?)
}

fn terminate_app_server(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn start_codex_app_server(data_home: &Path) -> Option<Child> {
    for binary in codex_binary_candidates() {
        let child = Command::new(binary)
            .args(["app-server", "--stdio"])
            .env("CODEX_HOME", data_home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn();
        if let Ok(child) = child {
            return Some(child);
        }
    }
    None
}

fn codex_binary_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(binary) = std::env::var_os("CODEX_BINARY").filter(|value| !value.is_empty()) {
        candidates.push(PathBuf::from(binary));
    }
    candidates.push(PathBuf::from("codex"));
    candidates.extend([
        PathBuf::from("/opt/homebrew/bin/codex"),
        PathBuf::from("/usr/local/bin/codex"),
        PathBuf::from("/usr/bin/codex"),
    ]);
    if let Some(user_dirs) = UserDirs::new() {
        candidates.push(user_dirs.home_dir().join(".local/bin/codex"));
        candidates.push(user_dirs.home_dir().join(".npm-global/bin/codex"));
    }
    candidates
}

fn parse_weekly_usage(response: &Value) -> Option<WeeklyUsage> {
    let mut snapshots = Vec::new();
    if let Some(snapshot) = response.pointer("/rateLimitsByLimitId/codex") {
        snapshots.push(snapshot);
    }
    if let Some(snapshot) = response.get("rateLimits") {
        snapshots.push(snapshot);
    }
    if let Some(by_id) = response
        .get("rateLimitsByLimitId")
        .and_then(Value::as_object)
    {
        snapshots.extend(by_id.values());
    }

    for snapshot in snapshots {
        for key in ["primary", "secondary"] {
            let Some(window) = snapshot.get(key) else {
                continue;
            };
            if window.get("windowDurationMins").and_then(Value::as_i64)
                != Some(WEEKLY_WINDOW_MINUTES)
            {
                continue;
            }
            let used_percent = window.get("usedPercent")?.as_f64()?.clamp(0.0, 100.0);
            return Some(WeeklyUsage {
                used_percent,
                remaining_percent: 100.0 - used_percent,
                resets_at: window.get("resetsAt").and_then(Value::as_i64),
            });
        }
    }
    None
}

fn requires_polling_watcher(path: &Path) -> bool {
    cfg!(target_os = "windows") && path.to_string_lossy().starts_with(r"\\")
}

fn catalog_number(value: Option<&serde_json::Value>) -> Option<String> {
    value?.as_number().map(ToString::to_string)
}

pub fn resolve_codex_home() -> Result<PathBuf, String> {
    if let Some(value) = std::env::var_os("CODEX_HOME").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(value));
    }
    let user_dirs = UserDirs::new().ok_or_else(|| "无法确定用户主目录".to_string())?;
    Ok(user_dirs.home_dir().join(".codex"))
}

pub fn app_project_dirs() -> Result<ProjectDirs, String> {
    ProjectDirs::from("io.github", "ericsmoon", "QuotaLoom")
        .ok_or_else(|| "无法确定应用数据目录".to_string())
}

/// 多 Home 配置：sources.json 是唯一事实来源。
/// 首次（文件不存在）按旧 codex-home.json + 三个知名默认目录播种。
fn source_homes_path() -> Result<PathBuf, String> {
    Ok(app_project_dirs()?.config_local_dir().join("sources.json"))
}

fn load_source_homes() -> Result<Vec<PathBuf>, String> {
    if let Some(path) = std::env::var_os("CODEX_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
    {
        // 显式 CODEX_HOME 保持单源语义
        return Ok(vec![path]);
    }
    let config_path = source_homes_path()?;
    if config_path.exists() {
        let data = std::fs::read(&config_path).map_err(|error| error.to_string())?;
        let homes: Vec<String> = serde_json::from_slice(&data).unwrap_or_default();
        let parsed: Vec<PathBuf> = homes
            .into_iter()
            .map(PathBuf::from)
            .filter(|home| home.is_dir())
            .collect();
        if !parsed.is_empty() {
            return Ok(parsed);
        }
    }
    let mut seeded: Vec<PathBuf> = Vec::new();
    if let Some(primary) = load_codex_home_preference().filter(|path| path.is_dir()) {
        seeded.push(primary);
    }
    if let Ok(default_home) = resolve_codex_home() {
        seeded.push(default_home);
    }
    if let Some(user_dirs) = UserDirs::new() {
        let home = user_dirs.home_dir().to_path_buf();
        for candidate in [home.join(".claude"), home.join(".zcode")] {
            if candidate.is_dir() && !seeded.contains(&candidate) {
                seeded.push(candidate);
            }
        }
    }
    let seeded: Vec<PathBuf> = seeded.into_iter().filter(|home| home.is_dir()).collect();
    if seeded.is_empty() {
        return Err("未找到任何可用的数据目录，请先添加".to_string());
    }
    let _ = save_source_homes(&seeded);
    Ok(seeded)
}

fn save_source_homes(homes: &[PathBuf]) -> Result<(), String> {
    let config_path = source_homes_path()?;
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let payload: Vec<String> = homes
        .iter()
        .map(|home| home.to_string_lossy().to_string())
        .collect();
    std::fs::write(
        &config_path,
        serde_json::to_vec(&payload).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())
}

fn load_codex_home_preference() -> Option<PathBuf> {
    let data = std::fs::read(codex_home_preference_path().ok()?).ok()?;
    serde_json::from_slice::<String>(&data)
        .ok()
        .map(PathBuf::from)
}

fn codex_home_preference_path() -> Result<PathBuf, String> {
    Ok(app_project_dirs()?
        .config_local_dir()
        .join("codex-home.json"))
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ZcodeQuotaPreference {
    plan_name: String,
    tokens_per_day: u64,
    #[serde(default)]
    reset_hour_local: Option<u32>,
}

fn zcode_quota_preference_path() -> Result<PathBuf, String> {
    Ok(app_project_dirs()?
        .config_local_dir()
        .join("zcode-quota.json"))
}

fn load_zcode_quota_preference() -> Option<ZcodeQuotaPreference> {
    let data = std::fs::read(zcode_quota_preference_path().ok()?).ok()?;
    serde_json::from_slice(&data).ok()
}

fn build_quota_estimate(
    preference: &ZcodeQuotaPreference,
    used_tokens: u64,
    now_local: chrono::DateTime<chrono::Local>,
) -> QuotaEstimate {
    let used_percent = if preference.tokens_per_day > 0 {
        used_tokens as f64 / preference.tokens_per_day as f64 * 100.0
    } else {
        0.0
    };
    let reset_hour = preference.reset_hour_local.unwrap_or(0).min(23);
    let resets_at = next_local_reset(&now_local, reset_hour);
    QuotaEstimate {
        plan_name: preference.plan_name.clone(),
        tokens_per_day: preference.tokens_per_day,
        used_tokens,
        used_percent,
        remaining_percent: (100.0 - used_percent).clamp(0.0, 100.0),
        resets_at,
    }
}

fn next_local_reset(now_local: &chrono::DateTime<chrono::Local>, reset_hour: u32) -> i64 {
    let today_reset = now_local
        .date_naive()
        .and_hms_opt(reset_hour, 0, 0)
        .and_then(|time| time.and_local_timezone(chrono::Local).earliest())
        .map(|time| time.timestamp());
    match today_reset {
        Some(reset) if reset > now_local.timestamp() => reset,
        _ => now_local
            .date_naive()
            .succ_opt()
            .and_then(|date| date.and_hms_opt(reset_hour, 0, 0))
            .and_then(|time| time.and_local_timezone(chrono::Local).earliest())
            .map(|time| time.timestamp())
            .unwrap_or_else(|| now_local.timestamp() + 86_400),
    }
}

fn session_roots(data_home: &Path, source_kind: DataSourceKind) -> Vec<PathBuf> {
    match source_kind {
        DataSourceKind::ClaudeCode => vec![data_home.join("projects")],
        DataSourceKind::CodexCli | DataSourceKind::ChatGptCodex => vec![
            data_home.join("sessions"),
            data_home.join("archived_sessions"),
        ],
        DataSourceKind::ZCode => {
            let cli_rollout = data_home.join("cli").join("rollout");
            if cli_rollout.is_dir() {
                vec![cli_rollout]
            } else if data_home.join("rollout").is_dir() {
                vec![data_home.join("rollout")]
            } else {
                vec![data_home.to_path_buf()]
            }
        }
        // All 只是快照聚合用的伪来源，不参与目录扫描
        DataSourceKind::All => Vec::new(),
    }
}

fn collect_session_files(data_home: &Path, source_kind: DataSourceKind) -> Vec<PathBuf> {
    if source_kind == DataSourceKind::ZCode {
        // 优先读 ZCode 本地数据库（结构化、无 64MB 轮换损失）
        if let Some(db_path) = find_zcode_db(data_home) {
            return vec![db_path];
        }
        // 旧版 ZCode 没有本地数据库：退回 rollout jsonl
    }
    let mut files = Vec::new();
    for root in session_roots(data_home, source_kind) {
        if !root.exists() {
            continue;
        }
        for entry in WalkDir::new(root)
            .follow_links(false)
            .max_depth(12)
            .into_iter()
            .filter_map(Result::ok)
        {
            if entry.file_type().is_file()
                && entry
                    .path()
                    .extension()
                    .and_then(|extension| extension.to_str())
                    == Some("jsonl")
            {
                files.push(entry.into_path());
            }
        }
    }
    files.sort();
    files
}

fn find_zcode_db(data_home: &Path) -> Option<PathBuf> {
    let mut candidates = vec![
        data_home.join("cli").join("db").join(ZCODE_DB_FILE_NAME),
        data_home.join("db").join(ZCODE_DB_FILE_NAME),
    ];
    if let Some(parent) = data_home.parent() {
        candidates.push(parent.join("db").join(ZCODE_DB_FILE_NAME));
    }
    candidates.into_iter().find(|candidate| candidate.is_file())
}

pub fn detect_data_source(path: &Path) -> DataSourceKind {
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if name == ".zcode"
        || name == "zcode"
        || name == "rollout"
        || path.join("cli").join("rollout").is_dir()
        || path.join("rollout").is_dir()
    {
        return DataSourceKind::ZCode;
    }
    if name == ".claude"
        || name == "claude"
        || path.join("projects").is_dir()
        || path.join("file-history").is_dir()
    {
        return DataSourceKind::ClaudeCode;
    }
    if name == ".chatgpt"
        || name == "chatgpt"
        || name.contains("chatgpt")
        || path.join("chatgpt").is_dir()
    {
        return DataSourceKind::ChatGptCodex;
    }
    if path.join("sessions").is_dir() && contains_chatgpt_session_metadata(path) {
        return DataSourceKind::ChatGptCodex;
    }
    DataSourceKind::CodexCli
}

fn contains_chatgpt_session_metadata(data_home: &Path) -> bool {
    for entry in WalkDir::new(data_home.join("sessions"))
        .follow_links(false)
        .max_depth(5)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry.file_type().is_file()
                && entry
                    .path()
                    .extension()
                    .and_then(|extension| extension.to_str())
                    == Some("jsonl")
        })
        .take(8)
    {
        let Ok(content) = std::fs::read_to_string(entry.path()) else {
            continue;
        };
        for line in content.lines().filter(|line| line.contains("session_meta")) {
            let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            let payload = value.get("payload").unwrap_or(&value);
            let originator = payload
                .get("originator")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_ascii_lowercase();
            let source = payload
                .get("source")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_ascii_lowercase();
            if originator.contains("chatgpt")
                || originator.contains("codex-app")
                || source == "app"
                || source == "desktop"
            {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    use std::fs::{self, OpenOptions};
    use std::io::Write;
    use tempfile::TempDir;

    fn line(value: Value) -> String {
        format!("{}\n", serde_json::to_string(&value).unwrap())
    }

    fn token_line(timestamp: &str, input: u64, cached: u64, output: u64) -> String {
        line(json!({
            "type": "event_msg",
            "timestamp": timestamp,
            "payload": {
                "type": "token_count",
                "info": {
                    "total_token_usage": {
                        "input_tokens": input,
                        "cached_input_tokens": cached,
                        "output_tokens": output
                    }
                }
            }
        }))
    }

    fn all_time() -> UsageRange {
        UsageRange {
            start_at: 0,
            end_at: i64::MAX,
            bucket_seconds: 3600,
            timezone_offset_seconds: 0,
        }
    }

    #[test]
    fn reads_weekly_window_from_either_rate_limit_slot() {
        let primary = parse_weekly_usage(&json!({
            "rateLimits": {
                "primary": {
                    "usedPercent": 12.5,
                    "windowDurationMins": 10080,
                    "resetsAt": 1_800_000_000
                },
                "secondary": null
            }
        }))
        .unwrap();
        assert_eq!(primary.used_percent, 12.5);
        assert_eq!(primary.remaining_percent, 87.5);
        assert_eq!(primary.resets_at, Some(1_800_000_000));

        let secondary = parse_weekly_usage(&json!({
            "rateLimits": {
                "primary": {
                    "usedPercent": 40,
                    "windowDurationMins": 300,
                    "resetsAt": 1_700_000_000
                },
                "secondary": {
                    "usedPercent": 21,
                    "windowDurationMins": 10080,
                    "resetsAt": 1_800_000_001
                }
            }
        }))
        .unwrap();
        assert_eq!(secondary.remaining_percent, 79.0);
        assert_eq!(secondary.resets_at, Some(1_800_000_001));
    }

    #[test]
    fn hides_rate_limits_without_a_weekly_window() {
        let usage = parse_weekly_usage(&json!({
            "rateLimits": {
                "primary": {
                    "usedPercent": 40,
                    "windowDurationMins": 300,
                    "resetsAt": 1_700_000_000
                },
                "secondary": null
            }
        }));
        assert_eq!(usage, None);
    }

    #[test]
    fn syncs_appends_without_duplicates_and_rebuilds_truncated_files() {
        let temp = TempDir::new().unwrap();
        let codex_home = temp.path().join("codex");
        let sessions = codex_home.join("sessions/2026/07/14");
        fs::create_dir_all(&sessions).unwrap();
        let session_path = sessions.join("rollout-thread-a.jsonl");
        let initial = format!(
            "{}{}{}",
            line(json!({"type":"session_meta","payload":{"id":"thread-a"}})),
            line(json!({"type":"turn_context","payload":{"model":"gpt-5.3-codex"}})),
            token_line("2026-07-14T08:00:00Z", 100, 40, 20),
        );
        fs::write(&session_path, initial).unwrap();

        let service = UsageService::open(codex_home, temp.path().join("usage.sqlite3")).unwrap();
        let first = service.sync().unwrap();
        assert_eq!(first.imported_events, 1);
        assert_eq!(
            service
                .snapshot(all_time(), DataSourceKind::CodexCli)
                .unwrap()
                .summary
                .total_tokens,
            120
        );

        let mut file = OpenOptions::new().append(true).open(&session_path).unwrap();
        file.write_all(token_line("2026-07-14T08:01:00Z", 250, 90, 55).as_bytes())
            .unwrap();
        file.flush().unwrap();

        let second = service.sync().unwrap();
        assert_eq!(second.imported_events, 1);
        let after_append = service
            .snapshot(all_time(), DataSourceKind::CodexCli)
            .unwrap();
        assert_eq!(after_append.summary.calls, 2);
        assert_eq!(after_append.summary.total_tokens, 305);
        assert_eq!(after_append.summary.fresh_input_tokens, 160);
        assert_eq!(after_append.summary.cached_input_tokens, 90);
        assert_eq!(after_append.summary.output_tokens, 55);

        let unchanged = service.sync().unwrap();
        assert_eq!(unchanged.changed_files, 0);
        assert_eq!(unchanged.imported_events, 0);
        assert_eq!(
            service
                .snapshot(all_time(), DataSourceKind::CodexCli)
                .unwrap()
                .summary
                .calls,
            2
        );

        let replacement = format!(
            "{}{}{}",
            line(json!({"type":"session_meta","payload":{"id":"thread-a"}})),
            line(json!({"type":"turn_context","payload":{"model":"gpt-5.3-codex"}})),
            token_line("2026-07-14T09:00:00Z", 50, 10, 5),
        );
        fs::write(&session_path, replacement).unwrap();

        let rebuilt = service.sync().unwrap();
        assert_eq!(rebuilt.rebuilt_files, 1);
        assert_eq!(rebuilt.imported_events, 1);
        let after_rebuild = service
            .snapshot(all_time(), DataSourceKind::CodexCli)
            .unwrap();
        assert_eq!(after_rebuild.summary.calls, 1);
        assert_eq!(after_rebuild.summary.total_tokens, 55);
        assert_eq!(after_rebuild.summary.fresh_input_tokens, 40);
        assert_eq!(after_rebuild.summary.cached_input_tokens, 10);
        assert_eq!(after_rebuild.summary.output_tokens, 5);
    }

    fn zcode_usage_line(timestamp: &str, input: u64, cache_read: u64, output: u64) -> String {
        line(json!({
            "type": "model_io",
            "sessionId": "sess_zcode",
            "startedAt": timestamp,
            "model": {"modelId": "GLM-5.3-Flash"},
            "response": {"usage": {
                "inputTokens": input,
                "cacheReadTokens": cache_read,
                "cacheWriteTokens": 0,
                "outputTokens": output,
                "totalTokens": input + output
            }}
        }))
    }

    #[test]
    fn detects_zcode_homes_at_multiple_depths() {
        let temp = TempDir::new().unwrap();
        let zcode_home = temp.path().join(".zcode");
        let cli = zcode_home.join("cli");
        let rollout = cli.join("rollout");
        fs::create_dir_all(&rollout).unwrap();
        assert_eq!(detect_data_source(&zcode_home), DataSourceKind::ZCode);
        assert_eq!(detect_data_source(&cli), DataSourceKind::ZCode);
        assert_eq!(detect_data_source(&rollout), DataSourceKind::ZCode);
        assert_eq!(
            session_roots(&zcode_home, DataSourceKind::ZCode),
            vec![rollout.clone()]
        );
        assert_eq!(
            session_roots(&cli, DataSourceKind::ZCode),
            vec![rollout.clone()]
        );
        assert_eq!(
            session_roots(&rollout, DataSourceKind::ZCode),
            vec![rollout.clone()]
        );

        let codex_home = temp.path().join(".codex");
        fs::create_dir_all(codex_home.join("sessions")).unwrap();
        assert_eq!(detect_data_source(&codex_home), DataSourceKind::CodexCli);
    }

    #[test]
    fn syncs_zcode_rollout_appends_without_duplicates_and_rebuilds_rotations() {
        let temp = TempDir::new().unwrap();
        let zcode_home = temp.path().join(".zcode");
        let rollout = zcode_home.join("cli/rollout");
        fs::create_dir_all(&rollout).unwrap();
        let session_path = rollout.join("model-io-sess_zcode.jsonl");
        fs::write(
            &session_path,
            format!(
                "{}{}",
                zcode_usage_line("2026-07-14T08:00:00.000Z", 100, 40, 20),
                zcode_usage_line("2026-07-14T08:01:00.000Z", 150, 50, 35),
            ),
        )
        .unwrap();

        let service = UsageService::open(zcode_home, temp.path().join("usage.sqlite3")).unwrap();
        let first = service.sync().unwrap();
        assert_eq!(first.imported_events, 2);
        let snapshot = service.snapshot(all_time(), DataSourceKind::ZCode).unwrap();
        assert_eq!(snapshot.source_kind, DataSourceKind::ZCode);
        assert_eq!(snapshot.source_label, "ZCode");
        assert_eq!(snapshot.summary.calls, 2);
        // input 已含缓存：100 + 150，缓存 40 + 50 只是其中拆分
        assert_eq!(snapshot.summary.total_tokens, 305);
        assert_eq!(snapshot.summary.fresh_input_tokens, 160);
        assert_eq!(snapshot.summary.cached_input_tokens, 90);
        assert_eq!(snapshot.summary.output_tokens, 55);

        let mut file = OpenOptions::new().append(true).open(&session_path).unwrap();
        file.write_all(zcode_usage_line("2026-07-14T08:02:00.000Z", 30, 0, 10).as_bytes())
            .unwrap();
        file.flush().unwrap();

        let second = service.sync().unwrap();
        assert_eq!(second.imported_events, 1);
        assert_eq!(
            service
                .snapshot(all_time(), DataSourceKind::ZCode)
                .unwrap()
                .summary
                .calls,
            3
        );

        // 64MB 轮换会覆盖同名文件：按截断处理，全量重建
        fs::write(
            &session_path,
            zcode_usage_line("2026-07-14T09:00:00.000Z", 50, 10, 5),
        )
        .unwrap();

        let rebuilt = service.sync().unwrap();
        assert_eq!(rebuilt.rebuilt_files, 1);
        assert_eq!(rebuilt.imported_events, 1);
        let after_rebuild = service.snapshot(all_time(), DataSourceKind::ZCode).unwrap();
        assert_eq!(after_rebuild.summary.calls, 1);
        // input = 50（已含 10 缓存读），total = 50 + 5 输出
        assert_eq!(after_rebuild.summary.total_tokens, 55);
        assert_eq!(after_rebuild.summary.fresh_input_tokens, 40);
        assert_eq!(after_rebuild.summary.cached_input_tokens, 10);
        assert_eq!(after_rebuild.summary.output_tokens, 5);
    }

    #[test]
    fn syncs_zcode_database_preferring_db_over_rollout() {
        let temp = TempDir::new().unwrap();
        let zcode_home = temp.path().join(".zcode");
        let rollout_dir = zcode_home.join("cli/rollout");
        fs::create_dir_all(&rollout_dir).unwrap();
        fs::write(
            rollout_dir.join("model-io-sess_legacy.jsonl"),
            zcode_usage_line("2026-07-14T08:00:00.000Z", 100_000, 0, 0),
        )
        .unwrap();

        let service =
            UsageService::open(zcode_home.clone(), temp.path().join("usage.sqlite3")).unwrap();
        let rollout_only = service.sync().unwrap();
        assert_eq!(rollout_only.imported_events, 1);
        assert_eq!(
            service
                .snapshot(all_time(), DataSourceKind::ZCode)
                .unwrap()
                .summary
                .total_tokens,
            100_000
        );

        // ZCode 本地数据库出现：收集层只认 db，且 db 源首启时清理 legacy jsonl 统计
        let db_path = zcode_home.join("cli/db/db.sqlite");
        fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        let connection = rusqlite::Connection::open(&db_path).unwrap();
        connection
            .execute_batch(
                "create table session (id text primary key, directory text, path text);
                insert into session values ('sess_z', 'D:\\Work\\demo', 'D:\\Work\\demo');
                create table model_usage (
                    id text primary key,
                    session_id text,
                    model_id text,
                    started_at integer,
                    input_tokens integer,
                    output_tokens integer,
                    cache_read_input_tokens integer,
                    cache_creation_input_tokens integer
                );
                insert into model_usage values
                    ('u1', 'sess_z', 'GLM-5.3-Flash', 1784016000000, 50, 5, 10, 0),
                    ('u2', 'sess_z', 'GLM-5.3-Flash', 1784016060000, 60, 7, 0, 0);",
            )
            .unwrap();
        drop(connection);

        let second = service.sync().unwrap();
        assert_eq!(second.scanned_files, 1);
        assert_eq!(second.imported_events, 2);
        let snapshot = service.snapshot(all_time(), DataSourceKind::ZCode).unwrap();
        assert_eq!(snapshot.summary.calls, 2);
        assert_eq!(snapshot.summary.total_tokens, 122);
        assert_eq!(snapshot.summary.cached_input_tokens, 10);
        assert_eq!(snapshot.summary.output_tokens, 12);
    }

    #[test]
    fn syncs_multiple_homes_independently_and_aggregates() {
        let temp = TempDir::new().unwrap();
        let codex_home = temp.path().join(".codex");
        let sessions = codex_home.join("sessions");
        fs::create_dir_all(&sessions).unwrap();
        fs::write(
            sessions.join("rollout-thread-codex.jsonl"),
            format!(
                "{}{}{}",
                line(json!({"type":"session_meta","payload":{"id":"thread-codex"}})),
                line(json!({"type":"turn_context","payload":{"model":"gpt-5.3-codex"}})),
                token_line("2026-07-14T08:00:00Z", 100, 0, 20),
            ),
        )
        .unwrap();
        let zcode_home = temp.path().join(".zcode");
        let rollout = zcode_home.join("cli/rollout");
        fs::create_dir_all(&rollout).unwrap();
        fs::write(
            rollout.join("model-io-sess_z.jsonl"),
            zcode_usage_line("2026-07-14T08:01:00.000Z", 50, 10, 5),
        )
        .unwrap();

        let service = UsageService::open_homes(
            vec![codex_home, zcode_home],
            temp.path().join("usage.sqlite3"),
        )
        .unwrap();
        let result = service.sync().unwrap();
        assert_eq!(result.imported_events, 2);

        let all = service.snapshot(all_time(), DataSourceKind::All).unwrap();
        assert_eq!(all.source_kind, DataSourceKind::All);
        assert_eq!(all.source_label, "全部来源");
        assert_eq!(all.summary.calls, 2);
        assert_eq!(all.summary.total_tokens, 175);

        let codex_only = service
            .snapshot(all_time(), DataSourceKind::CodexCli)
            .unwrap();
        assert_eq!(codex_only.summary.calls, 1);
        assert_eq!(codex_only.summary.total_tokens, 120);

        let zcode_only = service.snapshot(all_time(), DataSourceKind::ZCode).unwrap();
        assert_eq!(zcode_only.summary.calls, 1);
        assert_eq!(zcode_only.summary.total_tokens, 55);
    }

    #[test]
    fn removes_source_home_and_purges_its_events() {
        let temp = TempDir::new().unwrap();
        let zcode_home = temp.path().join(".zcode");
        let rollout = zcode_home.join("cli/rollout");
        fs::create_dir_all(&rollout).unwrap();
        fs::write(
            rollout.join("model-io-sess_z.jsonl"),
            zcode_usage_line("2026-07-14T08:00:00.000Z", 50, 0, 5),
        )
        .unwrap();
        let codex_home = temp.path().join(".codex");
        let sessions = codex_home.join("sessions");
        fs::create_dir_all(&sessions).unwrap();
        fs::write(
            sessions.join("rollout-thread-keep.jsonl"),
            format!(
                "{}{}{}",
                line(json!({"type":"session_meta","payload":{"id":"thread-keep"}})),
                line(json!({"type":"turn_context","payload":{"model":"gpt-5.3-codex"}})),
                token_line("2026-07-14T08:01:00Z", 30, 0, 3),
            ),
        )
        .unwrap();

        let service = UsageService::open_homes(
            vec![zcode_home.clone(), codex_home],
            temp.path().join("usage.sqlite3"),
        )
        .unwrap();
        service.sync().unwrap();
        assert_eq!(
            service
                .snapshot(all_time(), DataSourceKind::All)
                .unwrap()
                .summary
                .calls,
            2
        );

        service.remove_source_home(&zcode_home).unwrap();
        // 被移除 Home 的事件被清理，其他来源不受影响
        assert_eq!(
            service
                .snapshot(all_time(), DataSourceKind::ZCode)
                .unwrap()
                .summary
                .calls,
            0
        );
        assert_eq!(
            service
                .snapshot(all_time(), DataSourceKind::CodexCli)
                .unwrap()
                .summary
                .calls,
            1
        );
    }

    #[test]
    fn builds_zcode_quota_estimate_with_future_reset() {
        let preference = ZcodeQuotaPreference {
            plan_name: "ZCode Weekend Build".to_string(),
            tokens_per_day: 300_000_000,
            reset_hour_local: None,
        };
        let now = chrono::Local::now();
        let estimate = build_quota_estimate(&preference, 155_000_000, now);
        assert_eq!(estimate.plan_name, "ZCode Weekend Build");
        assert!((estimate.used_percent - 155.0 / 300.0 * 100.0).abs() < 1e-9);
        assert!((estimate.remaining_percent - 145.0 / 300.0 * 100.0).abs() < 1e-9);
        assert!(estimate.resets_at > now.timestamp());
    }

    #[test]
    fn quota_estimate_remaining_never_negative() {
        let preference = ZcodeQuotaPreference {
            plan_name: "plan".to_string(),
            tokens_per_day: 100,
            reset_hour_local: Some(9),
        };
        let estimate = build_quota_estimate(&preference, 250, chrono::Local::now());
        assert!((estimate.used_percent - 250.0).abs() < 1e-9);
        assert_eq!(estimate.remaining_percent, 0.0);
        assert!(estimate.resets_at > 0);
    }
}
