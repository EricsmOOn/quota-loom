use crate::core::types::{DataSourceKind, ParseOutcome, SessionCursor, TokenTotals, UsageEvent};
use chrono::DateTime;
use rusqlite::{Connection, OpenFlags};
use serde_json::Value;
use std::fs;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

pub const ZCODE_DB_FILE_NAME: &str = "db.sqlite";

pub fn is_zcode_db_file(path: &Path) -> bool {
    path.file_name().and_then(|name| name.to_str()) == Some(ZCODE_DB_FILE_NAME)
}

/// 游标与事件的键：完整文件路径（多 Home 下避免不同来源同名文件互踩）。
pub fn source_key(path: &Path) -> String {
    path.to_string_lossy().to_string()
}

/// 线程展示兜底名：文件名。
fn display_key(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

/// 项目名：取路径（/ 或 \ 分隔）最后一个非空段。
fn project_from_path(raw: &str) -> String {
    raw.trim_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .find(|segment| !segment.is_empty())
        .unwrap_or_default()
        .to_string()
}

pub fn normalize_model(raw: &str) -> String {
    let mut model = raw.trim().to_ascii_lowercase();
    if let Some((_, suffix)) = model.rsplit_once('/') {
        model = suffix.to_string();
    }
    for suffix_len in [11, 9] {
        if model.len() <= suffix_len || !model.is_char_boundary(model.len() - suffix_len) {
            continue;
        }
        let suffix = &model[model.len() - suffix_len..];
        let is_date = if suffix_len == 11 {
            let bytes = suffix.as_bytes();
            bytes[0] == b'-'
                && bytes[1..5].iter().all(u8::is_ascii_digit)
                && bytes[5] == b'-'
                && bytes[6..8].iter().all(u8::is_ascii_digit)
                && bytes[8] == b'-'
                && bytes[9..11].iter().all(u8::is_ascii_digit)
        } else {
            suffix.as_bytes()[0] == b'-' && suffix.as_bytes()[1..].iter().all(u8::is_ascii_digit)
        };
        if is_date {
            model.truncate(model.len() - suffix_len);
            break;
        }
    }
    if model.is_empty() {
        "unknown".to_string()
    } else {
        model
    }
}

pub fn parse_session_file(
    path: &Path,
    previous_cursor: Option<SessionCursor>,
) -> Result<ParseOutcome, String> {
    let metadata = fs::metadata(path)
        .map_err(|error| format!("无法读取会话文件元数据 {}: {error}", path.display()))?;
    let file_size = metadata.len();
    let modified_ns = metadata
        .modified()
        .ok()
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .map(|value| value.as_nanos().min(u64::MAX as u128) as u64)
        .unwrap_or(0);
    let key = source_key(path);
    let reset_required = previous_cursor
        .as_ref()
        .is_some_and(|cursor| file_size < cursor.byte_offset);
    let mut cursor = if reset_required {
        SessionCursor::default()
    } else {
        previous_cursor.unwrap_or_default()
    };
    cursor.source_key = key.clone();
    cursor.path = path.to_string_lossy().to_string();
    if cursor.current_model.is_empty() {
        cursor.current_model = "unknown".to_string();
    }

    if !reset_required && cursor.file_size == file_size && cursor.modified_ns == modified_ns {
        return Ok(ParseOutcome {
            cursor,
            events: Vec::new(),
            reset_required: false,
        });
    }

    let mut file = fs::File::open(path)
        .map_err(|error| format!("无法打开会话文件 {}: {error}", path.display()))?;
    file.seek(SeekFrom::Start(cursor.byte_offset))
        .map_err(|error| format!("无法定位会话文件 {}: {error}", path.display()))?;
    let mut reader = BufReader::new(file);
    let mut events = Vec::new();
    let mut consumed_offset = cursor.byte_offset;

    loop {
        let mut bytes = Vec::new();
        let read = reader
            .read_until(b'\n', &mut bytes)
            .map_err(|error| format!("读取会话文件失败 {}: {error}", path.display()))?;
        if read == 0 {
            break;
        }
        if !bytes.ends_with(b"\n") {
            break;
        }
        consumed_offset = consumed_offset.saturating_add(read as u64);
        let Ok(line) = std::str::from_utf8(&bytes) else {
            continue;
        };
        process_line(line.trim_end(), &key, path, &mut cursor, &mut events);
    }

    cursor.byte_offset = consumed_offset;
    cursor.file_size = file_size;
    cursor.modified_ns = modified_ns;

    Ok(ParseOutcome {
        cursor,
        events,
        reset_required,
    })
}

pub fn parse_session_file_for_source(
    path: &Path,
    previous_cursor: Option<SessionCursor>,
    source_kind: DataSourceKind,
) -> Result<ParseOutcome, String> {
    match source_kind {
        DataSourceKind::ClaudeCode => parse_claude_session_file(path, previous_cursor),
        DataSourceKind::CodexCli | DataSourceKind::ChatGptCodex => {
            parse_session_file(path, previous_cursor)
        }
        DataSourceKind::ZCode => parse_zcode_session_file(path, previous_cursor),
        // All 只是快照聚合用的伪来源，不参与文件解析
        DataSourceKind::All => Err("All 不是可解析的数据源".to_string()),
    }
}

fn parse_claude_session_file(
    path: &Path,
    previous_cursor: Option<SessionCursor>,
) -> Result<ParseOutcome, String> {
    let metadata = fs::metadata(path)
        .map_err(|error| format!("无法读取会话文件元数据 {}: {error}", path.display()))?;
    let file_size = metadata.len();
    let modified_ns = metadata
        .modified()
        .ok()
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .map(|value| value.as_nanos().min(u64::MAX as u128) as u64)
        .unwrap_or(0);
    let key = source_key(path);
    let reset_required = previous_cursor
        .as_ref()
        .is_some_and(|cursor| file_size < cursor.byte_offset);
    let mut cursor = if reset_required {
        SessionCursor::default()
    } else {
        previous_cursor.unwrap_or_default()
    };
    cursor.source_key = key.clone();
    cursor.path = path.to_string_lossy().to_string();
    if cursor.current_model.is_empty() {
        cursor.current_model = "unknown".to_string();
    }

    if !reset_required && cursor.file_size == file_size && cursor.modified_ns == modified_ns {
        return Ok(ParseOutcome {
            cursor,
            events: Vec::new(),
            reset_required: false,
        });
    }

    let mut file = fs::File::open(path)
        .map_err(|error| format!("无法打开会话文件 {}: {error}", path.display()))?;
    file.seek(SeekFrom::Start(cursor.byte_offset))
        .map_err(|error| format!("无法定位会话文件 {}: {error}", path.display()))?;
    let mut reader = BufReader::new(file);
    let mut events = Vec::new();
    let mut consumed_offset = cursor.byte_offset;

    loop {
        let mut bytes = Vec::new();
        let read = reader
            .read_until(b'\n', &mut bytes)
            .map_err(|error| format!("读取会话文件失败 {}: {error}", path.display()))?;
        if read == 0 {
            break;
        }
        if !bytes.ends_with(b"\n") {
            break;
        }
        consumed_offset = consumed_offset.saturating_add(read as u64);
        let Ok(line) = std::str::from_utf8(&bytes) else {
            continue;
        };
        process_claude_line(line.trim_end(), &key, path, &mut cursor, &mut events);
    }

    cursor.byte_offset = consumed_offset;
    cursor.file_size = file_size;
    cursor.modified_ns = modified_ns;

    Ok(ParseOutcome {
        cursor,
        events,
        reset_required,
    })
}

fn process_claude_line(
    line: &str,
    source_key: &str,
    path: &Path,
    cursor: &mut SessionCursor,
    events: &mut Vec<UsageEvent>,
) {
    let Ok(value) = serde_json::from_str::<Value>(line) else {
        return;
    };
    if value.get("type").and_then(Value::as_str) != Some("assistant") {
        return;
    }
    let Some(message) = value.get("message") else {
        return;
    };
    let Some(usage) = message.get("usage") else {
        return;
    };
    let input_tokens = number(usage.get("input_tokens")).unwrap_or(0);
    let cached_input_tokens = number(usage.get("cache_read_input_tokens")).unwrap_or(0);
    let cache_creation_tokens = number(usage.get("cache_creation_input_tokens")).unwrap_or(0);
    let output_tokens = number(usage.get("output_tokens")).unwrap_or(0);
    let delta = TokenTotals {
        input_tokens: input_tokens
            .saturating_add(cached_input_tokens)
            .saturating_add(cache_creation_tokens),
        cached_input_tokens,
        output_tokens,
    };
    if delta.is_zero() {
        return;
    }
    if let Some(model) = message.get("model").and_then(Value::as_str) {
        cursor.current_model = normalize_model(model);
    }
    let thread_id = value
        .get("sessionId")
        .or_else(|| value.get("session_id"))
        .or_else(|| value.get("sessionID"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .or_else(|| cursor.thread_id.clone())
        .unwrap_or_else(|| display_key(path));
    cursor.thread_id = Some(thread_id.clone());
    cursor.event_index = cursor.event_index.saturating_add(1);
    let occurred_at = value
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(|timestamp| DateTime::parse_from_rfc3339(timestamp).ok())
        .map(|timestamp| timestamp.timestamp())
        .unwrap_or_else(|| chrono::Utc::now().timestamp());
    let message_id = message.get("id").and_then(Value::as_str);
    let event_id = message_id
        .map(|id| format!("claude:{id}"))
        .unwrap_or_else(|| format!("claude:{thread_id}:{}", cursor.event_index));
    let project = value
        .get("cwd")
        .and_then(Value::as_str)
        .map(project_from_path)
        .filter(|project| !project.is_empty())
        .or_else(|| cursor.project.clone())
        .unwrap_or_default();
    if !project.is_empty() {
        cursor.project = Some(project.clone());
    }
    events.push(UsageEvent {
        id: event_id,
        source_key: source_key.to_string(),
        thread_id,
        event_index: cursor.event_index,
        occurred_at,
        model: cursor.current_model.clone(),
        tokens: delta,
        source_file: path.to_string_lossy().to_string(),
        project,
    });
}

fn parse_zcode_session_file(
    path: &Path,
    previous_cursor: Option<SessionCursor>,
) -> Result<ParseOutcome, String> {
    if is_zcode_db_file(path) {
        return parse_zcode_db_file(path, previous_cursor);
    }
    parse_zcode_rollout_file(path, previous_cursor)
}

fn parse_zcode_rollout_file(
    path: &Path,
    previous_cursor: Option<SessionCursor>,
) -> Result<ParseOutcome, String> {
    let metadata = fs::metadata(path)
        .map_err(|error| format!("无法读取会话文件元数据 {}: {error}", path.display()))?;
    let file_size = metadata.len();
    let modified_ns = metadata
        .modified()
        .ok()
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .map(|value| value.as_nanos().min(u64::MAX as u128) as u64)
        .unwrap_or(0);
    let key = source_key(path);
    let reset_required = previous_cursor
        .as_ref()
        .is_some_and(|cursor| file_size < cursor.byte_offset);
    let mut cursor = if reset_required {
        SessionCursor::default()
    } else {
        previous_cursor.unwrap_or_default()
    };
    cursor.source_key = key.clone();
    cursor.path = path.to_string_lossy().to_string();
    if cursor.current_model.is_empty() {
        cursor.current_model = "unknown".to_string();
    }

    if !reset_required && cursor.file_size == file_size && cursor.modified_ns == modified_ns {
        return Ok(ParseOutcome {
            cursor,
            events: Vec::new(),
            reset_required: false,
        });
    }

    let mut file = fs::File::open(path)
        .map_err(|error| format!("无法打开会话文件 {}: {error}", path.display()))?;
    file.seek(SeekFrom::Start(cursor.byte_offset))
        .map_err(|error| format!("无法定位会话文件 {}: {error}", path.display()))?;
    let mut reader = BufReader::new(file);
    let mut events = Vec::new();
    let mut consumed_offset = cursor.byte_offset;

    loop {
        let mut bytes = Vec::new();
        let read = reader
            .read_until(b'\n', &mut bytes)
            .map_err(|error| format!("读取会话文件失败 {}: {error}", path.display()))?;
        if read == 0 {
            break;
        }
        if !bytes.ends_with(b"\n") {
            break;
        }
        consumed_offset = consumed_offset.saturating_add(read as u64);
        let Ok(line) = std::str::from_utf8(&bytes) else {
            continue;
        };
        process_zcode_line(line.trim_end(), &key, path, &mut cursor, &mut events);
    }

    cursor.byte_offset = consumed_offset;
    cursor.file_size = file_size;
    cursor.modified_ns = modified_ns;

    Ok(ParseOutcome {
        cursor,
        events,
        reset_required,
    })
}

fn process_zcode_line(
    line: &str,
    source_key: &str,
    path: &Path,
    cursor: &mut SessionCursor,
    events: &mut Vec<UsageEvent>,
) {
    if !line.contains("\"model_io\"") || !line.contains("\"usage\"") {
        return;
    }
    let Ok(value) = serde_json::from_str::<Value>(line) else {
        return;
    };
    if value.get("type").and_then(Value::as_str) != Some("model_io") {
        return;
    }
    let Some(usage) = zcode_usage(&value) else {
        return;
    };
    // ZCode（AI SDK 口径）的 inputTokens 已经【包含】缓存读/写，
    // cacheRead/cacheWrite 只是 input 的子集拆分（实测：input+cacheRead 会超过
    // 1M 上下文窗口，而 ZCode 界面缓存命中率 ~95% 也只有该口径能对上）。
    // 因此 input 按原值入账，缓存部分仅作 cached 拆分展示。
    let input_tokens = number(usage.get("inputTokens"))
        .or_else(|| number(usage.get("input_tokens")))
        .unwrap_or(0);
    let cache_read_tokens = number(usage.get("cacheReadTokens"))
        .or_else(|| number(usage.get("cache_read_input_tokens")))
        .unwrap_or(0);
    let cache_write_tokens = number(usage.get("cacheWriteTokens"))
        .or_else(|| number(usage.get("cache_creation_input_tokens")))
        .unwrap_or(0);
    let output_tokens = number(usage.get("outputTokens"))
        .or_else(|| number(usage.get("output_tokens")))
        .unwrap_or(0);
    let cached_input_tokens = cache_read_tokens
        .saturating_add(cache_write_tokens)
        .min(input_tokens);
    let delta = TokenTotals {
        input_tokens,
        cached_input_tokens,
        output_tokens,
    };
    if delta.is_zero() {
        return;
    }
    if let Some(model) = value
        .pointer("/model/modelId")
        .or_else(|| value.pointer("/model/model"))
        .and_then(Value::as_str)
    {
        cursor.current_model = normalize_model(model);
    }
    let thread_id = value
        .get("sessionId")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .or_else(|| cursor.thread_id.clone())
        .unwrap_or_else(|| display_key(path));
    cursor.thread_id = Some(thread_id.clone());
    cursor.event_index = cursor.event_index.saturating_add(1);
    let occurred_at = value
        .get("startedAt")
        .and_then(Value::as_str)
        .and_then(|timestamp| DateTime::parse_from_rfc3339(timestamp).ok())
        .map(|timestamp| timestamp.timestamp())
        .unwrap_or_else(|| chrono::Utc::now().timestamp());
    let event_index = cursor.event_index;
    events.push(UsageEvent {
        id: format!("zcode:{thread_id}:{event_index}"),
        source_key: source_key.to_string(),
        thread_id,
        event_index,
        occurred_at,
        model: cursor.current_model.clone(),
        tokens: delta,
        source_file: path.to_string_lossy().to_string(),
        project: String::new(),
    });
}

fn zcode_usage(value: &Value) -> Option<&Value> {
    value
        .pointer("/response/usage")
        .filter(|usage| usage.is_object())
        .or_else(|| {
            value
                .pointer("/response/providerMetadata/anthropic/usage")
                .filter(|usage| usage.is_object())
        })
}

fn parse_zcode_db_file(
    path: &Path,
    previous_cursor: Option<SessionCursor>,
) -> Result<ParseOutcome, String> {
    let key = source_key(path);
    let mut cursor = previous_cursor.unwrap_or_default();
    cursor.source_key = key.clone();
    cursor.path = path.to_string_lossy().to_string();
    // SessionCursor.byte_offset 复用为「已处理的 model_usage.rowid 游标」
    let last_rowid = i64::try_from(cursor.byte_offset).unwrap_or(i64::MAX);

    let (connection, temp_dir) = open_zcode_db(path)?;
    let outcome = read_zcode_db_usage(&connection, last_rowid, &key, path, &mut cursor);
    drop(connection);
    if let Some(dir) = temp_dir {
        let _ = std::fs::remove_dir_all(dir);
    }
    outcome
}

/// ZCode 正在写入自己的数据库：优先只读直连，失败（持锁 / WAL 需恢复）则
/// 把 db 三件套拷到临时目录读副本，避免干扰客户端。
fn open_zcode_db(path: &Path) -> Result<(Connection, Option<PathBuf>), String> {
    if let Ok(connection) = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY) {
        let usable = connection
            .query_row("select count(*) from sqlite_master", [], |row| {
                row.get::<_, i64>(0)
            })
            .is_ok();
        if usable {
            return Ok((connection, None));
        }
    }
    let temp_dir = std::env::temp_dir().join(format!("quota-loom-zcode-db-{}", std::process::id()));
    std::fs::create_dir_all(&temp_dir)
        .map_err(|error| format!("无法创建 ZCode 数据库副本目录: {error}"))?;
    for suffix in ["", "-wal", "-shm"] {
        let mut source = path.to_path_buf();
        source.as_mut_os_string().push(suffix);
        if source.exists() {
            let mut target = temp_dir.join(ZCODE_DB_FILE_NAME);
            target.as_mut_os_string().push(suffix);
            std::fs::copy(&source, &target)
                .map_err(|error| format!("无法复制 ZCode 数据库 {}: {error}", source.display()))?;
        }
    }
    let connection = Connection::open_with_flags(
        temp_dir.join(ZCODE_DB_FILE_NAME),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .map_err(|error| format!("无法打开 ZCode 数据库副本: {error}"))?;
    Ok((connection, Some(temp_dir)))
}

struct ZcodeDbUsageRow {
    rowid: i64,
    session_id: Option<String>,
    model_id: Option<String>,
    started_at: Option<i64>,
    input_tokens: i64,
    cache_read_input_tokens: i64,
    cache_creation_input_tokens: i64,
    output_tokens: i64,
    directory: Option<String>,
    session_path: Option<String>,
}

fn read_zcode_db_usage(
    connection: &Connection,
    last_rowid: i64,
    key: &str,
    path: &Path,
    cursor: &mut SessionCursor,
) -> Result<ParseOutcome, String> {
    let has_table: bool = connection
        .query_row(
            "select exists(select 1 from sqlite_master where type='table' and name='model_usage')",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map(|value| value != 0)
        .unwrap_or(false);
    if !has_table {
        // 旧版 ZCode 没有该表：游标原位保持，事件为空（rollout jsonl 兜底由收集层负责）
        return Ok(ParseOutcome {
            cursor: std::mem::take(cursor),
            events: Vec::new(),
            reset_required: false,
        });
    }
    let max_rowid: i64 = connection
        .query_row(
            "select coalesce(max(rowid), 0) from model_usage",
            [],
            |row| row.get(0),
        )
        .map_err(|error| format!("读取 ZCode 数据库失败 {}: {error}", path.display()))?;

    let mut events = Vec::new();
    if max_rowid > last_rowid {
        let mut statement = connection
            .prepare(
                "select mu.rowid, mu.session_id, mu.model_id, mu.started_at,
                        mu.input_tokens, mu.cache_read_input_tokens,
                        mu.cache_creation_input_tokens, mu.output_tokens,
                        s.directory, s.path
                 from model_usage mu
                 left join session s on s.id = mu.session_id
                 where mu.rowid > ?1 and mu.rowid <= ?2
                 order by mu.rowid",
            )
            .map_err(|error| format!("读取 ZCode 数据库失败 {}: {error}", path.display()))?;
        let mapped = statement
            .query_map(rusqlite::params![last_rowid, max_rowid], |row| {
                Ok(ZcodeDbUsageRow {
                    rowid: row.get(0)?,
                    session_id: row.get(1)?,
                    model_id: row.get(2)?,
                    started_at: row.get(3)?,
                    input_tokens: row.get(4)?,
                    cache_read_input_tokens: row.get(5)?,
                    cache_creation_input_tokens: row.get(6)?,
                    output_tokens: row.get(7)?,
                    directory: row.get(8)?,
                    session_path: row.get(9)?,
                })
            })
            .map_err(|error| format!("读取 ZCode 数据库失败 {}: {error}", path.display()))?;
        let rows = mapped
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("读取 ZCode 数据库失败 {}: {error}", path.display()))?;
        for row in rows {
            let input_tokens = row.input_tokens.max(0) as u64;
            let cached = (row.cache_read_input_tokens.max(0))
                .saturating_add(row.cache_creation_input_tokens.max(0))
                as u64;
            let output_tokens = row.output_tokens.max(0) as u64;
            // 与 rollout 解析同口径：input 已含缓存读写，缓存只作 cached 拆分
            let totals = TokenTotals {
                input_tokens,
                cached_input_tokens: cached.min(input_tokens),
                output_tokens,
            };
            if totals.is_zero() {
                continue;
            }
            if let Some(model) = row.model_id.as_deref().filter(|model| !model.is_empty()) {
                cursor.current_model = normalize_model(model);
            }
            let thread_id = row
                .session_id
                .filter(|session| !session.is_empty())
                .unwrap_or_else(|| key.to_string());
            cursor.thread_id = Some(thread_id.clone());
            let project = row
                .directory
                .as_deref()
                .or(row.session_path.as_deref())
                .map(project_from_path)
                .filter(|project| !project.is_empty())
                .unwrap_or_default();
            if !project.is_empty() {
                cursor.project = Some(project.clone());
            }
            cursor.event_index = cursor.event_index.saturating_add(1);
            let occurred_at = match row.started_at {
                Some(millis) if millis > 0 => millis / 1000,
                _ => chrono::Utc::now().timestamp(),
            };
            events.push(UsageEvent {
                id: format!("zcode:db:{}", row.rowid),
                source_key: key.to_string(),
                thread_id,
                event_index: cursor.event_index,
                occurred_at,
                model: cursor.current_model.clone(),
                tokens: totals,
                source_file: path.to_string_lossy().to_string(),
                project,
            });
        }
    }
    // rowid 只增不减；若表被清空（max < last），保持游标原位，旧事件继续有效
    cursor.byte_offset = max_rowid.max(last_rowid) as u64;
    Ok(ParseOutcome {
        cursor: std::mem::take(cursor),
        events,
        reset_required: false,
    })
}

fn process_line(
    line: &str,
    source_key: &str,
    path: &Path,
    cursor: &mut SessionCursor,
    events: &mut Vec<UsageEvent>,
) {
    if line.is_empty() {
        return;
    }
    if !line.contains("session_meta")
        && !line.contains("turn_context")
        && !line.contains("token_count")
        && !line.contains("thread_settings_applied")
        && !line.contains("inter_agent_communication")
    {
        return;
    }
    let Ok(value) = serde_json::from_str::<Value>(line) else {
        return;
    };
    let event_type = value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();

    if event_type.starts_with("inter_agent_communication")
        || (event_type == "event_msg"
            && value.pointer("/payload/type").and_then(Value::as_str)
                == Some("thread_settings_applied"))
    {
        cursor.replay_active = false;
        return;
    }

    match event_type {
        "session_meta" => {
            if let Some(payload) = value.get("payload") {
                if let Some(identity) = session_identity(payload) {
                    if cursor.thread_id.is_none() {
                        cursor.thread_id = Some(identity.thread_id);
                    }
                    if cursor.event_index == 0 && identity.carries_history_snapshot {
                        cursor.replay_active = true;
                    }
                }
                if let Some(cwd) = payload
                    .get("cwd")
                    .or_else(|| payload.get("workspace_path"))
                    .and_then(Value::as_str)
                {
                    let project = project_from_path(cwd);
                    if !project.is_empty() {
                        cursor.project = Some(project);
                    }
                }
            }
        }
        "turn_context" => {
            if let Some(model) = value
                .pointer("/payload/model")
                .or_else(|| value.pointer("/payload/info/model"))
                .and_then(Value::as_str)
            {
                cursor.current_model = normalize_model(model);
            }
            if let Some(cwd) = value.pointer("/payload/cwd").and_then(Value::as_str) {
                let project = project_from_path(cwd);
                if !project.is_empty() {
                    cursor.project = Some(project);
                }
            }
        }
        "event_msg" => process_token_event(value, source_key, path, cursor, events),
        _ => {}
    }
}

fn process_token_event(
    value: Value,
    source_key: &str,
    path: &Path,
    cursor: &mut SessionCursor,
    events: &mut Vec<UsageEvent>,
) {
    if value.pointer("/payload/type").and_then(Value::as_str) != Some("token_count") {
        return;
    }
    let Some(info) = value
        .pointer("/payload/info")
        .filter(|value| !value.is_null())
    else {
        return;
    };
    if let Some(model) = info
        .get("model")
        .or_else(|| info.get("model_name"))
        .or_else(|| value.pointer("/payload/model"))
        .and_then(Value::as_str)
    {
        cursor.current_model = normalize_model(model);
    }

    let total = parse_totals(info.get("total_token_usage"));
    let last = parse_totals(info.get("last_token_usage"));
    let delta = match total {
        Some(total) => {
            let delta = cursor
                .previous_total
                .and_then(|previous| subtract_totals(total, previous))
                .or(last)
                .unwrap_or(total);
            cursor.previous_total = Some(total);
            delta
        }
        None => match last {
            Some(last) => last,
            None => return,
        },
    };
    let delta = TokenTotals {
        cached_input_tokens: delta.cached_input_tokens.min(delta.input_tokens),
        ..delta
    };
    if delta.is_zero() {
        return;
    }

    cursor.event_index = cursor.event_index.saturating_add(1);
    if cursor.replay_active {
        return;
    }
    let thread_id = cursor
        .thread_id
        .clone()
        .unwrap_or_else(|| source_key.to_string());
    let occurred_at = value
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(|timestamp| DateTime::parse_from_rfc3339(timestamp).ok())
        .map(|timestamp| timestamp.timestamp())
        .unwrap_or_else(|| chrono::Utc::now().timestamp());
    let event_index = cursor.event_index;
    events.push(UsageEvent {
        id: format!("codex:{thread_id}:{event_index}"),
        source_key: source_key.to_string(),
        thread_id,
        event_index,
        occurred_at,
        model: cursor.current_model.clone(),
        tokens: delta,
        source_file: path.to_string_lossy().to_string(),
        project: cursor.project.clone().unwrap_or_default(),
    });
}

fn subtract_totals(current: TokenTotals, previous: TokenTotals) -> Option<TokenTotals> {
    if current.input_tokens < previous.input_tokens
        || current.cached_input_tokens < previous.cached_input_tokens
        || current.output_tokens < previous.output_tokens
    {
        return None;
    }
    Some(TokenTotals {
        input_tokens: current.input_tokens - previous.input_tokens,
        cached_input_tokens: current.cached_input_tokens - previous.cached_input_tokens,
        output_tokens: current.output_tokens - previous.output_tokens,
    })
}

fn parse_totals(value: Option<&Value>) -> Option<TokenTotals> {
    let value = value?;
    Some(TokenTotals {
        input_tokens: number(value.get("input_tokens")).unwrap_or(0),
        cached_input_tokens: number(value.get("cached_input_tokens")).unwrap_or(0),
        output_tokens: number(value.get("output_tokens")).unwrap_or(0),
    })
}

fn number(value: Option<&Value>) -> Option<u64> {
    let value = value?;
    value
        .as_u64()
        .or_else(|| value.as_i64().and_then(|number| u64::try_from(number).ok()))
        .or_else(|| value.as_str().and_then(|number| number.parse().ok()))
}

struct SessionIdentity {
    thread_id: String,
    carries_history_snapshot: bool,
}

fn session_identity(payload: &Value) -> Option<SessionIdentity> {
    let thread_id = payload
        .get("id")
        .or_else(|| payload.get("thread_id"))
        .or_else(|| payload.get("threadId"))
        .or_else(|| payload.get("session_id"))
        .or_else(|| payload.get("sessionId"))
        .and_then(Value::as_str)?
        .to_string();
    let parent = payload
        .get("session_id")
        .or_else(|| payload.get("sessionId"))
        .and_then(Value::as_str);
    let carries_history_snapshot = payload
        .get("forked_from_id")
        .and_then(Value::as_str)
        .is_some_and(|value| !value.is_empty())
        || payload.pointer("/source/subagent").is_some()
        || parent.is_some_and(|parent| parent != thread_id);
    Some(SessionIdentity {
        thread_id,
        carries_history_snapshot,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::{NamedTempFile, TempDir};

    fn line(value: Value) -> String {
        format!("{}\n", serde_json::to_string(&value).unwrap())
    }

    #[test]
    fn parses_cumulative_deltas_and_model_changes() {
        let mut file = NamedTempFile::new().unwrap();
        write!(
            file,
            "{}{}{}{}",
            line(serde_json::json!({"type":"session_meta","payload":{"id":"thread-a"}})),
            line(serde_json::json!({"type":"turn_context","payload":{"model":"openai/gpt-5.3-codex-20260305"}})),
            line(serde_json::json!({"type":"event_msg","timestamp":"2026-07-14T08:00:00Z","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":40,"output_tokens":20},"last_token_usage":{"input_tokens":100,"cached_input_tokens":40,"output_tokens":20}}}})),
            line(serde_json::json!({"type":"event_msg","timestamp":"2026-07-14T08:01:00Z","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":250,"cached_input_tokens":90,"output_tokens":55},"last_token_usage":{"input_tokens":150,"cached_input_tokens":50,"output_tokens":35}}}})),
        )
        .unwrap();

        let outcome = parse_session_file(file.path(), None).unwrap();
        assert_eq!(outcome.events.len(), 2);
        assert_eq!(outcome.events[0].model, "gpt-5.3-codex");
        assert_eq!(outcome.events[1].tokens.input_tokens, 150);
        assert_eq!(outcome.events[1].tokens.cached_input_tokens, 50);
        assert_eq!(outcome.events[1].tokens.output_tokens, 35);
    }

    #[test]
    fn parses_claude_usage_records() {
        let mut file = NamedTempFile::new().unwrap();
        write!(
            file,
            "{}{}",
            line(serde_json::json!({
                "type":"user",
                "sessionId":"claude-session"
            })),
            line(serde_json::json!({
                "type":"assistant",
                "sessionId":"claude-session",
                "timestamp":"2026-07-15T08:00:00Z",
                "message":{
                    "id":"msg-1",
                    "model":"claude-sonnet-4-20250514",
                    "usage":{
                        "input_tokens":100,
                        "cache_read_input_tokens":40,
                        "cache_creation_input_tokens":10,
                        "output_tokens":20
                    }
                }
            }))
        )
        .unwrap();

        let outcome =
            parse_session_file_for_source(file.path(), None, DataSourceKind::ClaudeCode).unwrap();
        assert_eq!(outcome.events.len(), 1);
        assert_eq!(outcome.events[0].id, "claude:msg-1");
        assert_eq!(outcome.events[0].thread_id, "claude-session");
        assert_eq!(outcome.events[0].model, "claude-sonnet-4");
        assert_eq!(outcome.events[0].tokens.input_tokens, 150);
        assert_eq!(outcome.events[0].tokens.cached_input_tokens, 40);
        assert_eq!(outcome.events[0].tokens.output_tokens, 20);
    }

    #[test]
    fn ignores_incomplete_tail_until_newline_arrives() {
        let mut file = NamedTempFile::new().unwrap();
        write!(
            file,
            "{}",
            line(serde_json::json!({"type":"session_meta","payload":{"id":"thread-b"}}))
        )
        .unwrap();
        write!(file, "{{\"type\":\"event_msg\"").unwrap();

        let outcome = parse_session_file(file.path(), None).unwrap();
        assert!(outcome.events.is_empty());
        assert!(outcome.cursor.byte_offset < outcome.cursor.file_size);
    }

    #[test]
    fn skips_fork_history_before_takeover_boundary() {
        let mut file = NamedTempFile::new().unwrap();
        write!(
            file,
            "{}{}{}{}",
            line(serde_json::json!({"type":"session_meta","payload":{"id":"child","session_id":"parent","source":{"subagent":{}}}})),
            line(serde_json::json!({"type":"event_msg","timestamp":"2026-07-14T08:00:00Z","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":10}}}})),
            line(serde_json::json!({"type":"event_msg","payload":{"type":"thread_settings_applied"}})),
            line(serde_json::json!({"type":"event_msg","timestamp":"2026-07-14T08:02:00Z","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":180,"cached_input_tokens":30,"output_tokens":25},"last_token_usage":{"input_tokens":80,"cached_input_tokens":10,"output_tokens":15}}}})),
        )
        .unwrap();

        let outcome = parse_session_file(file.path(), None).unwrap();
        assert_eq!(outcome.events.len(), 1);
        assert_eq!(outcome.events[0].thread_id, "child");
        assert_eq!(outcome.events[0].tokens.input_tokens, 80);
    }

    fn zcode_usage_line(
        session_id: Option<&str>,
        input: u64,
        cache_read: u64,
        output: u64,
    ) -> String {
        let mut value = serde_json::json!({
            "type":"model_io",
            "startedAt":"2026-07-14T08:00:00.852Z",
            "model":{"modelId":"GLM-5.3-Flash","providerId":"builtin:bigmodel-start-plan"},
            "request":{"messages":[]},
            "response":{"usage":{
                "inputTokens":input,
                "cacheReadTokens":cache_read,
                "cacheWriteTokens":0,
                "outputTokens":output,
                "totalTokens":input + output
            }}
        });
        if let Some(session_id) = session_id {
            value["sessionId"] = serde_json::json!(session_id);
        }
        line(value)
    }

    #[test]
    fn parses_zcode_usage_records() {
        let mut file = NamedTempFile::new().unwrap();
        write!(
            file,
            "{}{}",
            zcode_usage_line(Some("sess_abc"), 100, 40, 20),
            zcode_usage_line(None, 10, 0, 3),
        )
        .unwrap();

        let outcome =
            parse_session_file_for_source(file.path(), None, DataSourceKind::ZCode).unwrap();
        assert_eq!(outcome.events.len(), 2);
        assert_eq!(outcome.events[0].id, "zcode:sess_abc:1");
        assert_eq!(outcome.events[0].thread_id, "sess_abc");
        assert_eq!(outcome.events[0].model, "glm-5.3-flash");
        assert_eq!(outcome.events[0].occurred_at, 1_784_016_000);
        // inputTokens 已含缓存读，缓存只作 cached 拆分
        assert_eq!(outcome.events[0].tokens.input_tokens, 100);
        assert_eq!(outcome.events[0].tokens.cached_input_tokens, 40);
        assert_eq!(outcome.events[0].tokens.output_tokens, 20);
        // 行内缺 sessionId 时沿用游标里最近一次的会话 ID
        assert_eq!(outcome.events[1].thread_id, "sess_abc");
        assert_eq!(outcome.events[1].tokens.input_tokens, 10);
        assert_eq!(outcome.events[1].tokens.output_tokens, 3);
    }

    #[test]
    fn falls_back_to_source_key_without_session_id() {
        let mut file = NamedTempFile::new().unwrap();
        write!(file, "{}", zcode_usage_line(None, 5, 0, 1)).unwrap();

        let outcome =
            parse_session_file_for_source(file.path(), None, DataSourceKind::ZCode).unwrap();
        assert_eq!(outcome.events.len(), 1);
        assert_eq!(
            outcome.events[0].thread_id,
            file.path().file_name().unwrap().to_str().unwrap()
        );
    }

    #[test]
    fn skips_zcode_lines_without_usage_and_counts_retries() {
        let mut file = NamedTempFile::new().unwrap();
        write!(
            file,
            "{}{}{}{}",
            line(serde_json::json!({
                "type":"model_io",
                "sessionId":"sess_err",
                "error":{"name":"ProviderBusinessError","message":"user concurrency limit exceeded"},
                "response":null
            })),
            line(serde_json::json!({
                "modelIOReset":{"maxFileBytes":67108864,"previousFileBytes":67129850,"reason":"session_file_size_limit"}
            })),
            zcode_usage_line(Some("sess_err"), 50, 10, 5),
            zcode_usage_line(Some("sess_err"), 60, 0, 7),
        )
        .unwrap();

        let outcome =
            parse_session_file_for_source(file.path(), None, DataSourceKind::ZCode).unwrap();
        assert_eq!(outcome.events.len(), 2);
        assert_eq!(outcome.events[0].thread_id, "sess_err");
        assert_eq!(outcome.events[0].tokens.input_tokens, 50);
        assert_eq!(outcome.events[1].tokens.input_tokens, 60);
    }

    #[test]
    fn falls_back_to_snake_case_provider_usage() {
        let mut file = NamedTempFile::new().unwrap();
        write!(
            file,
            "{}",
            line(serde_json::json!({
                "type":"model_io",
                "sessionId":"sess_snake",
                "response":{"providerMetadata":{"anthropic":{"usage":{
                    "input_tokens":200,
                    "cache_read_input_tokens":50,
                    "output_tokens":30
                }}}}
            }))
        )
        .unwrap();

        let outcome =
            parse_session_file_for_source(file.path(), None, DataSourceKind::ZCode).unwrap();
        assert_eq!(outcome.events.len(), 1);
        assert_eq!(outcome.events[0].tokens.input_tokens, 200);
        assert_eq!(outcome.events[0].tokens.cached_input_tokens, 50);
        assert_eq!(outcome.events[0].tokens.output_tokens, 30);
    }

    #[test]
    fn parses_zcode_db_usage_rows_incrementally() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("db.sqlite");
        let connection = Connection::open(&db_path).unwrap();
        connection
            .execute_batch(
                "create table session (id text primary key, directory text, path text);
                insert into session values ('sess_db', 'D:\\Workspace\\demo-project', 'D:\\Workspace\\demo-project');
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
                    ('u1', 'sess_db', 'GLM-5.3-Flash', 1784016000000, 100, 20, 40, 0),
                    ('u2', 'sess_db', 'GLM-5.3-Flash', 1784016060000, 10, 3, 0, 5);",
            )
            .unwrap();
        drop(connection);

        let outcome = parse_session_file_for_source(&db_path, None, DataSourceKind::ZCode).unwrap();
        assert_eq!(outcome.events.len(), 2);
        assert_eq!(outcome.events[0].id, "zcode:db:1");
        assert_eq!(outcome.events[0].thread_id, "sess_db");
        assert_eq!(outcome.events[0].project, "demo-project");
        assert_eq!(outcome.events[0].tokens.input_tokens, 100);
        assert_eq!(outcome.events[0].tokens.cached_input_tokens, 40);
        assert_eq!(outcome.events[1].tokens.input_tokens, 10);
        assert_eq!(outcome.events[1].tokens.cached_input_tokens, 5);
        assert_eq!(outcome.cursor.byte_offset, 2);

        let again = parse_session_file_for_source(
            &db_path,
            Some(outcome.cursor.clone()),
            DataSourceKind::ZCode,
        )
        .unwrap();
        assert!(again.events.is_empty());

        let connection = Connection::open(&db_path).unwrap();
        connection
            .execute(
                "insert into model_usage values
                    ('u3', 'sess_db', 'GLM-5.3-Flash', 1784016120000, 0, 0, 0, 0),
                    ('u4', 'sess_db', 'GLM-5.3-Flash', 1784016180000, 30, 4, 0, 0)",
                [],
            )
            .unwrap();
        drop(connection);

        let third =
            parse_session_file_for_source(&db_path, Some(outcome.cursor), DataSourceKind::ZCode)
                .unwrap();
        assert_eq!(third.events.len(), 1);
        assert_eq!(third.events[0].id, "zcode:db:4");
        assert_eq!(third.events[0].tokens.input_tokens, 30);
        assert_eq!(third.cursor.byte_offset, 4);
    }
}
