use base64::Engine as _;
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::models::cursor::{CursorAccount, CursorAccountIndex, CursorImportPayload};
use crate::modules::{account, logger};

const ACCOUNTS_INDEX_FILE: &str = "cursor_accounts.json";
const ACCOUNTS_DIR: &str = "cursor_accounts";
const CURSOR_QUOTA_ALERT_COOLDOWN_SECONDS: i64 = 10 * 60;
const CURSOR_ACCESS_TOKEN_REFRESH_THRESHOLD_SECONDS: i64 = 5 * 60;
/// 批量刷新并发上限：并行提速，同时限制峰值内存与 API 压力
const CURSOR_REFRESH_MAX_CONCURRENT: usize = 8;
/// FREE_CREDIT 事件汇总分页大小（较小页降低单账号峰值内存）
const FREE_CREDIT_USAGE_EVENTS_PAGE_SIZE: i32 = 100;

lazy_static::lazy_static! {
    static ref CURSOR_ACCOUNT_INDEX_LOCK: Mutex<()> = Mutex::new(());
    static ref CURSOR_QUOTA_ALERT_LAST_SENT: Mutex<HashMap<String, i64>> = Mutex::new(HashMap::new());
    static ref CURSOR_HTTP_CLIENT: reqwest::Client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .pool_max_idle_per_host(CURSOR_REFRESH_MAX_CONCURRENT)
        .build()
        .expect("failed to build shared Cursor HTTP client");
}

fn now_ts() -> i64 {
    chrono::Utc::now().timestamp()
}

fn normalize_status_value(value: Option<&str>) -> Option<String> {
    value.and_then(|raw| {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_ascii_lowercase())
        }
    })
}

fn is_banned_status(value: Option<&str>) -> bool {
    matches!(
        normalize_status_value(value).as_deref(),
        Some("banned") | Some("ban") | Some("forbidden")
    )
}

fn is_banned_reason(value: Option<&str>) -> bool {
    let Some(reason) = normalize_status_value(value) else {
        return false;
    };
    reason.contains("banned")
        || reason.contains("forbidden")
        || reason.contains("suspended")
        || reason.contains("disabled")
        || reason.contains("封禁")
        || reason.contains("禁用")
}

pub(crate) fn is_banned_account(account: &CursorAccount) -> bool {
    is_banned_status(account.status.as_deref())
        || is_banned_reason(account.status_reason.as_deref())
}

// ---------------------------------------------------------------------------
// Storage helpers
// ---------------------------------------------------------------------------

fn get_data_dir() -> Result<PathBuf, String> {
    account::get_data_dir()
}

fn get_accounts_dir() -> Result<PathBuf, String> {
    let base = get_data_dir()?;
    let dir = base.join(ACCOUNTS_DIR);
    if !dir.exists() {
        fs::create_dir_all(&dir).map_err(|e| format!("创建 Cursor 账号目录失败: {}", e))?;
    }
    Ok(dir)
}

fn get_accounts_index_path() -> Result<PathBuf, String> {
    Ok(get_data_dir()?.join(ACCOUNTS_INDEX_FILE))
}

pub fn accounts_index_path_string() -> Result<String, String> {
    Ok(get_accounts_index_path()?.to_string_lossy().to_string())
}

fn normalize_account_id(account_id: &str) -> Result<String, String> {
    let trimmed = account_id.trim();
    if trimmed.is_empty() {
        return Err("账号 ID 不能为空".to_string());
    }

    if trimmed.contains('/') || trimmed.contains('\\') || trimmed.contains("..") {
        return Err("账号 ID 非法，包含路径字符".to_string());
    }

    let valid = trimmed
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' || ch == '.');
    if !valid {
        return Err("账号 ID 非法，仅允许字母/数字/._-".to_string());
    }

    Ok(trimmed.to_string())
}

fn resolve_account_file_path(account_id: &str) -> Result<PathBuf, String> {
    let normalized = normalize_account_id(account_id)?;
    Ok(get_accounts_dir()?.join(format!("{}.json", normalized)))
}

// ---------------------------------------------------------------------------
// Account file operations
// ---------------------------------------------------------------------------

pub fn load_account(account_id: &str) -> Option<CursorAccount> {
    let account_path = resolve_account_file_path(account_id).ok()?;
    if !account_path.exists() {
        return None;
    }
    let content = fs::read_to_string(&account_path).ok()?;
    crate::modules::atomic_write::parse_json_with_auto_restore(&account_path, &content).ok()
}

fn save_account_file(account: &CursorAccount) -> Result<(), String> {
    let path = resolve_account_file_path(account.id.as_str())?;
    let content =
        serde_json::to_string_pretty(account).map_err(|e| format!("序列化账号失败: {}", e))?;
    crate::modules::atomic_write::write_string_atomic(&path, &content)
        .map_err(|e| format!("保存账号失败: {}", e))
}

fn delete_account_file(account_id: &str) -> Result<(), String> {
    let path = resolve_account_file_path(account_id)?;
    if path.exists() {
        fs::remove_file(path).map_err(|e| format!("删除账号文件失败: {}", e))?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Index operations
// ---------------------------------------------------------------------------

fn load_account_index() -> CursorAccountIndex {
    let path = match get_accounts_index_path() {
        Ok(p) => p,
        Err(_) => return CursorAccountIndex::new(),
    };

    if !path.exists() {
        return CursorAccountIndex::new();
    }

    match fs::read_to_string(path.as_path()) {
        Ok(content) => match crate::modules::atomic_write::parse_json_with_auto_restore::<
            CursorAccountIndex,
        >(&path, &content)
        {
            Ok(index) => index,
            Err(err) => {
                logger::log_warn(&format!(
                    "[Cursor Account] 账号索引解析失败，使用空索引兜底: path={}, error={}",
                    path.display(),
                    err
                ));
                CursorAccountIndex::new()
            }
        },
        Err(err) => {
            logger::log_warn(&format!(
                "[Cursor Account] 读取账号索引失败，使用空索引兜底: path={}, error={}",
                path.display(),
                err
            ));
            CursorAccountIndex::new()
        }
    }
}

fn load_account_index_checked() -> Result<CursorAccountIndex, String> {
    let path = get_accounts_index_path()?;
    if !path.exists() {
        return Ok(CursorAccountIndex::new());
    }

    let content = match fs::read_to_string(path.as_path()) {
        Ok(content) => content,
        Err(err) => {
            if !collect_account_ids_from_directory().is_empty() {
                logger::log_warn(&format!(
                    "[Cursor Account] 读取账号索引失败，将按账号目录补扫恢复: path={}, error={}",
                    path.display(),
                    err
                ));
                return Ok(CursorAccountIndex::new());
            }
            return Err(format!("读取账号索引失败: {}", err));
        }
    };

    if content.trim().is_empty() {
        return Ok(CursorAccountIndex::new());
    }

    match crate::modules::atomic_write::parse_json_with_auto_restore::<CursorAccountIndex>(
        &path, &content,
    ) {
        Ok(index) => Ok(index),
        Err(err) => {
            if !collect_account_ids_from_directory().is_empty() {
                logger::log_warn(&format!(
                    "[Cursor Account] 账号索引解析失败，将按账号目录补扫恢复: path={}, error={}",
                    path.display(),
                    err
                ));
                return Ok(CursorAccountIndex::new());
            }
            Err(crate::error::file_corrupted_error(
                ACCOUNTS_INDEX_FILE,
                &path.to_string_lossy(),
                &err.to_string(),
            ))
        }
    }
}

fn save_account_index(index: &CursorAccountIndex) -> Result<(), String> {
    let path = get_accounts_index_path()?;
    let content =
        serde_json::to_string_pretty(index).map_err(|e| format!("序列化账号索引失败: {}", e))?;
    crate::modules::atomic_write::write_string_atomic(&path, &content)
        .map_err(|e| format!("写入账号索引失败: {}", e))
}

fn refresh_summary(index: &mut CursorAccountIndex, account: &CursorAccount) {
    if let Some(summary) = index.accounts.iter_mut().find(|item| item.id == account.id) {
        *summary = account.summary();
        return;
    }
    index.accounts.push(account.summary());
}

fn upsert_account_record(account: CursorAccount) -> Result<CursorAccount, String> {
    let _lock = CURSOR_ACCOUNT_INDEX_LOCK
        .lock()
        .map_err(|_| "获取 Cursor 账号锁失败".to_string())?;
    let mut index = load_account_index();
    save_account_file(&account)?;
    refresh_summary(&mut index, &account);
    save_account_index(&index)?;
    Ok(account)
}

/// 刷新写回时合并刷新期间可能被改动的本地字段（如标签），避免长耗时刷新覆盖用户编辑。
fn upsert_refreshed_account_record(mut account: CursorAccount) -> Result<CursorAccount, String> {
    let _lock = CURSOR_ACCOUNT_INDEX_LOCK
        .lock()
        .map_err(|_| "获取 Cursor 账号锁失败".to_string())?;
    if let Some(latest) = load_account(&account.id) {
        account.tags = latest.tags;
        account.session_notes = latest.session_notes;
    }
    let mut index = load_account_index();
    save_account_file(&account)?;
    refresh_summary(&mut index, &account);
    save_account_index(&index)?;
    Ok(account)
}

fn persist_quota_query_error(account_id: &str, message: &str) {
    let Some(mut account) = load_account(account_id) else {
        return;
    };
    account.quota_query_last_error = Some(message.to_string());
    account.quota_query_last_error_at = Some(chrono::Utc::now().timestamp_millis());
    let _ = upsert_account_record(account);
}

// ---------------------------------------------------------------------------
// Identity helpers
// ---------------------------------------------------------------------------

fn normalize_non_empty(value: Option<&str>) -> Option<String> {
    value.and_then(|raw| {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

fn normalize_email_identity(value: Option<&str>) -> Option<String> {
    normalize_non_empty(value).and_then(|raw| {
        let lowered = raw.to_lowercase();
        if lowered.contains('@') {
            Some(lowered)
        } else {
            None
        }
    })
}

fn normalize_token_identity(value: Option<&str>) -> Option<String> {
    normalize_non_empty(value)
}

fn normalize_auth_identity(value: Option<&str>) -> Option<String> {
    normalize_non_empty(value)
}

fn decode_access_token_payload(access_token: &str) -> Option<serde_json::Value> {
    let parts: Vec<&str> = access_token.split('.').collect();
    if parts.len() < 2 {
        return None;
    }

    let payload_b64 = parts[1].replace('-', "+").replace('_', "/");
    let padded = match payload_b64.len() % 4 {
        2 => format!("{}==", payload_b64),
        3 => format!("{}=", payload_b64),
        _ => payload_b64,
    };

    let decoded = base64::engine::general_purpose::STANDARD
        .decode(padded)
        .ok()?;
    serde_json::from_slice(&decoded).ok()
}

fn extract_auth_id_from_access_token(access_token: &str) -> Option<String> {
    let value = decode_access_token_payload(access_token)?;
    normalize_non_empty(value.get("sub").and_then(|raw| raw.as_str()))
}

fn extract_access_token_exp(access_token: &str) -> Option<i64> {
    let value = decode_access_token_payload(access_token)?;
    value.get("exp").and_then(|raw| raw.as_i64())
}

fn access_token_needs_refresh(access_token: &str) -> bool {
    let Some(exp) = extract_access_token_exp(access_token) else {
        return true;
    };
    exp <= now_ts() + CURSOR_ACCESS_TOKEN_REFRESH_THRESHOLD_SECONDS
}

fn extract_auth_id_from_raw_value(raw: Option<&Value>) -> Option<String> {
    let obj = raw.and_then(|value| value.as_object())?;

    normalize_auth_identity(
        obj.get("authId")
            .and_then(|value| value.as_str())
            .or_else(|| obj.get("auth_id").and_then(|value| value.as_str()))
            .or_else(|| obj.get("workosId").and_then(|value| value.as_str()))
            .or_else(|| obj.get("workos_id").and_then(|value| value.as_str())),
    )
}

fn resolve_payload_auth_id(payload: &CursorImportPayload) -> Option<String> {
    normalize_auth_identity(payload.auth_id.as_deref())
        .or_else(|| extract_auth_id_from_raw_value(payload.cursor_auth_raw.as_ref()))
        .or_else(|| extract_auth_id_from_access_token(payload.access_token.as_str()))
}

fn resolve_account_auth_id(account: &CursorAccount) -> Option<String> {
    normalize_auth_identity(account.auth_id.as_deref())
        .or_else(|| extract_auth_id_from_raw_value(account.cursor_auth_raw.as_ref()))
        .or_else(|| extract_auth_id_from_access_token(account.access_token.as_str()))
}

fn cursor_auth_raw_object_mut(account: &mut CursorAccount) -> &mut serde_json::Map<String, Value> {
    if !matches!(account.cursor_auth_raw, Some(Value::Object(_))) {
        account.cursor_auth_raw = Some(Value::Object(serde_json::Map::new()));
    }

    match account.cursor_auth_raw.as_mut() {
        Some(Value::Object(obj)) => obj,
        _ => unreachable!("cursor_auth_raw 应始终为对象"),
    }
}

fn upsert_cursor_auth_raw_string(account: &mut CursorAccount, key: &str, value: Option<String>) {
    let Some(text) = normalize_non_empty(value.as_deref()) else {
        return;
    };
    cursor_auth_raw_object_mut(account).insert(key.to_string(), Value::String(text));
}

fn upsert_cursor_auth_raw_bool(account: &mut CursorAccount, key: &str, value: Option<bool>) {
    let Some(flag) = value else {
        return;
    };
    cursor_auth_raw_object_mut(account).insert(key.to_string(), Value::Bool(flag));
}

fn normalize_cursor_sign_up_type(value: Option<&str>) -> Option<String> {
    let raw = normalize_non_empty(value)?;
    match raw.as_str() {
        "SIGN_UP_TYPE_AUTH_0" => Some("Auth_0".to_string()),
        "SIGN_UP_TYPE_GOOGLE" => Some("Google".to_string()),
        "SIGN_UP_TYPE_GITHUB" => Some("Github".to_string()),
        "SIGN_UP_TYPE_WORKOS" => Some("WorkOS".to_string()),
        _ => Some(raw),
    }
}

fn accounts_are_duplicates(left: &CursorAccount, right: &CursorAccount) -> bool {
    let left_auth_id = resolve_account_auth_id(left);
    let right_auth_id = resolve_account_auth_id(right);
    if let (Some(left_auth), Some(right_auth)) = (left_auth_id.as_ref(), right_auth_id.as_ref()) {
        return left_auth == right_auth;
    }
    if left_auth_id.is_some() || right_auth_id.is_some() {
        return false;
    }

    let left_email = normalize_email_identity(Some(left.email.as_str()));
    let right_email = normalize_email_identity(Some(right.email.as_str()));
    let left_token = normalize_token_identity(Some(left.access_token.as_str()));
    let right_token = normalize_token_identity(Some(right.access_token.as_str()));

    let email_conflict = matches!(
        (left_email.as_ref(), right_email.as_ref()),
        (Some(l), Some(r)) if l != r
    );
    if email_conflict {
        return false;
    }

    let email_match = matches!(
        (left_email.as_ref(), right_email.as_ref()),
        (Some(l), Some(r)) if l == r
    );
    let token_match = matches!(
        (left_token.as_ref(), right_token.as_ref()),
        (Some(l), Some(r)) if l == r
    );

    email_match || token_match
}

// ---------------------------------------------------------------------------
// Merge helpers
// ---------------------------------------------------------------------------

fn merge_string_list(
    primary: Option<Vec<String>>,
    secondary: Option<Vec<String>>,
) -> Option<Vec<String>> {
    let mut merged = Vec::new();
    let mut seen = HashSet::new();

    for source in [primary, secondary] {
        if let Some(values) = source {
            for value in values {
                let trimmed = value.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let key = trimmed.to_lowercase();
                if seen.insert(key) {
                    merged.push(trimmed.to_string());
                }
            }
        }
    }

    if merged.is_empty() {
        None
    } else {
        Some(merged)
    }
}

const CURSOR_SESSION_NOTE_MAX_CHARS: usize = 120;

fn merge_string_map(
    primary: Option<HashMap<String, String>>,
    secondary: Option<HashMap<String, String>>,
) -> Option<HashMap<String, String>> {
    let mut merged = HashMap::new();
    for source in [primary, secondary] {
        if let Some(values) = source {
            for (key, value) in values {
                let trimmed_key = key.trim();
                let trimmed_value = value.trim();
                if trimmed_key.is_empty() || trimmed_value.is_empty() {
                    continue;
                }
                merged.entry(trimmed_key.to_string()).or_insert_with(|| {
                    trimmed_value.chars().take(CURSOR_SESSION_NOTE_MAX_CHARS).collect()
                });
            }
        }
    }
    if merged.is_empty() {
        None
    } else {
        Some(merged)
    }
}

fn fill_if_empty_string(target: &mut String, source: &str) {
    if target.trim().is_empty() {
        let incoming = source.trim();
        if !incoming.is_empty() {
            *target = incoming.to_string();
        }
    }
}

fn fill_if_none<T: Clone>(target: &mut Option<T>, source: &Option<T>) {
    if target.is_none() {
        *target = source.clone();
    }
}

fn merge_duplicate_account(primary: &mut CursorAccount, duplicate: &CursorAccount) {
    fill_if_empty_string(&mut primary.email, duplicate.email.as_str());
    fill_if_empty_string(&mut primary.access_token, duplicate.access_token.as_str());

    fill_if_none(&mut primary.auth_id, &duplicate.auth_id);
    fill_if_none(&mut primary.name, &duplicate.name);
    fill_if_none(&mut primary.refresh_token, &duplicate.refresh_token);
    fill_if_none(&mut primary.membership_type, &duplicate.membership_type);
    fill_if_none(
        &mut primary.subscription_status,
        &duplicate.subscription_status,
    );
    fill_if_none(&mut primary.sign_up_type, &duplicate.sign_up_type);
    fill_if_none(&mut primary.cursor_auth_raw, &duplicate.cursor_auth_raw);
    fill_if_none(&mut primary.cursor_usage_raw, &duplicate.cursor_usage_raw);
    fill_if_none(
        &mut primary.cursor_welcome_back_raw,
        &duplicate.cursor_welcome_back_raw,
    );
    fill_if_none(&mut primary.status, &duplicate.status);
    fill_if_none(&mut primary.status_reason, &duplicate.status_reason);

    primary.tags = merge_string_list(primary.tags.clone(), duplicate.tags.clone());
    primary.session_notes = merge_string_map(
        primary.session_notes.clone(),
        duplicate.session_notes.clone(),
    );
    primary.created_at = primary.created_at.min(duplicate.created_at);
    primary.last_used = primary.last_used.max(duplicate.last_used);
}

fn choose_primary_account_index(group: &[usize], accounts: &[CursorAccount]) -> usize {
    group
        .iter()
        .copied()
        .max_by(|left, right| {
            let left_account = &accounts[*left];
            let right_account = &accounts[*right];
            left_account
                .last_used
                .cmp(&right_account.last_used)
                .then_with(|| right_account.created_at.cmp(&left_account.created_at))
        })
        .unwrap_or(group[0])
}

fn collect_account_ids_from_directory() -> Vec<String> {
    let accounts_dir = match get_accounts_dir() {
        Ok(dir) => dir,
        Err(err) => {
            logger::log_warn(&format!(
                "[Cursor Account] 获取账号目录失败，跳过目录补扫: {}",
                err
            ));
            return Vec::new();
        }
    };

    let entries = match fs::read_dir(&accounts_dir) {
        Ok(value) => value,
        Err(err) => {
            logger::log_warn(&format!(
                "[Cursor Account] 读取账号目录失败，跳过目录补扫: path={}, error={}",
                accounts_dir.display(),
                err
            ));
            return Vec::new();
        }
    };

    let mut ids = Vec::new();
    for entry in entries {
        let Ok(item) = entry else {
            continue;
        };
        let path = item.path();
        if !path.is_file() {
            continue;
        }

        let is_json = path
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.eq_ignore_ascii_case("json"))
            .unwrap_or(false);
        if !is_json {
            continue;
        }

        let Some(stem) = path.file_stem().and_then(|name| name.to_str()) else {
            continue;
        };
        let Ok(account_id) = normalize_account_id(stem) else {
            logger::log_warn(&format!(
                "[Cursor Account] 检测到非法账号文件名，已忽略: file={}",
                path.display()
            ));
            continue;
        };
        ids.push(account_id);
    }

    ids.sort();
    ids.dedup();
    ids
}

fn normalize_account_index(index: &mut CursorAccountIndex) -> Vec<CursorAccount> {
    let mut loaded_accounts = Vec::new();
    let mut seen_account_ids = HashSet::new();
    let mut seen_summary_ids = HashSet::new();

    for summary in &index.accounts {
        if !seen_summary_ids.insert(summary.id.clone()) {
            continue;
        }
        if let Some(account) = load_account(&summary.id) {
            if seen_account_ids.insert(account.id.clone()) {
                loaded_accounts.push(account);
            }
        }
    }

    let mut recovered_count = 0usize;
    for account_id in collect_account_ids_from_directory() {
        if seen_account_ids.contains(&account_id) {
            continue;
        }
        if let Some(account) = load_account(&account_id) {
            if seen_account_ids.insert(account.id.clone()) {
                if !seen_summary_ids.contains(&account_id) {
                    recovered_count += 1;
                }
                loaded_accounts.push(account);
            }
        }
    }
    if recovered_count > 0 {
        logger::log_warn(&format!(
            "[Cursor Account] 检测到索引缺失，已从账号目录恢复 {} 个账号",
            recovered_count
        ));
    }

    if loaded_accounts.len() <= 1 {
        index.accounts = loaded_accounts
            .iter()
            .map(|account| account.summary())
            .collect();
        return loaded_accounts;
    }

    let mut parents: Vec<usize> = (0..loaded_accounts.len()).collect();

    fn find(parents: &mut [usize], idx: usize) -> usize {
        let parent = parents[idx];
        if parent == idx {
            return idx;
        }
        let root = find(parents, parent);
        parents[idx] = root;
        root
    }

    fn union(parents: &mut [usize], left: usize, right: usize) {
        let left_root = find(parents, left);
        let right_root = find(parents, right);
        if left_root != right_root {
            parents[right_root] = left_root;
        }
    }

    let total = loaded_accounts.len();
    for left in 0..total {
        for right in (left + 1)..total {
            if accounts_are_duplicates(&loaded_accounts[left], &loaded_accounts[right]) {
                union(&mut parents, left, right);
            }
        }
    }

    let mut grouped: HashMap<usize, Vec<usize>> = HashMap::new();
    for idx in 0..total {
        let root = find(&mut parents, idx);
        grouped.entry(root).or_default().push(idx);
    }

    let mut processed_roots = HashSet::new();
    let mut normalized_accounts = Vec::new();
    let mut removed_ids = Vec::new();
    for idx in 0..total {
        let root = find(&mut parents, idx);
        if !processed_roots.insert(root) {
            continue;
        }
        let Some(group) = grouped.get(&root) else {
            continue;
        };

        if group.len() == 1 {
            normalized_accounts.push(loaded_accounts[group[0]].clone());
            continue;
        }

        let primary_idx = choose_primary_account_index(group, &loaded_accounts);
        let mut primary = loaded_accounts[primary_idx].clone();
        for member in group {
            if *member == primary_idx {
                continue;
            }
            merge_duplicate_account(&mut primary, &loaded_accounts[*member]);
            removed_ids.push(loaded_accounts[*member].id.clone());
        }

        normalized_accounts.push(primary);
    }

    if !removed_ids.is_empty() {
        for account in &normalized_accounts {
            if let Err(err) = save_account_file(account) {
                logger::log_warn(&format!(
                    "[Cursor Account] 保存去重账号失败: id={}, error={}",
                    account.id, err
                ));
            }
        }
        for account_id in &removed_ids {
            if let Err(err) = delete_account_file(account_id) {
                logger::log_warn(&format!(
                    "[Cursor Account] 删除重复账号文件失败: id={}, error={}",
                    account_id, err
                ));
            }
        }
        logger::log_warn(&format!(
            "[Cursor Account] 检测到重复账号并已合并: removed_ids={}",
            removed_ids.join(",")
        ));
    }

    index.accounts = normalized_accounts
        .iter()
        .map(|account| account.summary())
        .collect();
    normalized_accounts
}

// ---------------------------------------------------------------------------
// CRUD
// ---------------------------------------------------------------------------

pub fn list_accounts() -> Vec<CursorAccount> {
    let _lock = CURSOR_ACCOUNT_INDEX_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut index = load_account_index();
    let had_index_accounts = !index.accounts.is_empty();
    let accounts = normalize_account_index(&mut index);
    if had_index_accounts && accounts.is_empty() {
        logger::log_warn(
            "[Cursor Account] 账号索引中存在账号，但详情文件均无法读取，已跳过空索引写回",
        );
        return accounts;
    }
    if let Err(err) = save_account_index(&index) {
        logger::log_warn(&format!("[Cursor Account] 保存账号索引失败: {}", err));
    }
    accounts
}

pub fn list_accounts_checked() -> Result<Vec<CursorAccount>, String> {
    let _lock = CURSOR_ACCOUNT_INDEX_LOCK
        .lock()
        .map_err(|_| "获取 Cursor 账号锁失败".to_string())?;
    let mut index = load_account_index_checked()?;
    let had_index_accounts = !index.accounts.is_empty();
    let accounts = normalize_account_index(&mut index);
    if had_index_accounts && accounts.is_empty() {
        return Err("Cursor 账号索引中存在账号，但详情文件均无法读取；已保留前端缓存，请从账号备份或本地账号文件恢复。".to_string());
    }
    if let Err(err) = save_account_index(&index) {
        logger::log_warn(&format!("[Cursor Account] 保存账号索引失败: {}", err));
    }
    Ok(accounts)
}

fn apply_payload(
    account: &mut CursorAccount,
    payload: CursorImportPayload,
    resolved_auth_id: Option<String>,
) {
    let incoming_email = payload.email.trim().to_string();
    if !incoming_email.is_empty() {
        account.email = incoming_email;
    } else if !account.email.contains('@') {
        account.email.clear();
    }
    account.name = payload.name;
    account.access_token = payload.access_token;
    account.refresh_token = payload.refresh_token;
    account.membership_type = payload.membership_type;
    account.subscription_status = payload.subscription_status;
    account.sign_up_type = payload.sign_up_type;
    account.cursor_auth_raw = payload.cursor_auth_raw;
    account.cursor_usage_raw = payload.cursor_usage_raw;
    if payload.cursor_credit_grants_raw.is_some() {
        account.cursor_credit_grants_raw = payload.cursor_credit_grants_raw;
    }
    if payload.cursor_free_credit_usage_raw.is_some() {
        account.cursor_free_credit_usage_raw = payload.cursor_free_credit_usage_raw;
    }
    if let Some(auth_id) = resolved_auth_id {
        account.auth_id = Some(auth_id.clone());
        upsert_cursor_auth_raw_string(account, "authId", Some(auth_id));
    }
    account.status = payload.status;
    account.status_reason = payload.status_reason;
    if let Some(tags) = payload.tags {
        account.tags = normalize_exported_tags(tags);
    }
    account.last_used = now_ts();
}

pub fn upsert_account(payload: CursorImportPayload) -> Result<CursorAccount, String> {
    let _lock = CURSOR_ACCOUNT_INDEX_LOCK
        .lock()
        .map_err(|_| "获取 Cursor 账号锁失败".to_string())?;

    let now = now_ts();
    let mut index = load_account_index();
    let incoming_auth_id = resolve_payload_auth_id(&payload);
    let incoming_email = normalize_email_identity(Some(payload.email.as_str()));
    let incoming_token = normalize_token_identity(Some(payload.access_token.as_str()));

    let identity_seed = incoming_auth_id
        .clone()
        .or_else(|| incoming_email.clone())
        .or_else(|| incoming_token.clone())
        .unwrap_or_else(|| "cursor_user".to_string())
        .to_lowercase();
    let generated_id = format!("cursor_{:x}", md5::compute(identity_seed.as_bytes()));

    let account_id = index
        .accounts
        .iter()
        .filter_map(|item| load_account(&item.id))
        .find(|account| {
            let existing_auth_id = resolve_account_auth_id(account);
            if let (Some(existing), Some(incoming)) =
                (existing_auth_id.as_ref(), incoming_auth_id.as_ref())
            {
                return existing == incoming;
            }
            if existing_auth_id.is_some() || incoming_auth_id.is_some() {
                return false;
            }

            let existing_email = normalize_email_identity(Some(account.email.as_str()));
            let existing_token = normalize_token_identity(Some(account.access_token.as_str()));
            if let (Some(ex), Some(inc)) = (existing_email.as_ref(), incoming_email.as_ref()) {
                if ex == inc {
                    return true;
                }
            }
            if let (Some(ex), Some(inc)) = (existing_token.as_ref(), incoming_token.as_ref()) {
                if ex == inc {
                    return true;
                }
            }
            false
        })
        .map(|account| account.id)
        .unwrap_or(generated_id);

    let existing = load_account(&account_id);
    let tags = existing.as_ref().and_then(|acc| acc.tags.clone());
    let session_notes = existing.as_ref().and_then(|acc| acc.session_notes.clone());
    let created_at = existing.as_ref().map(|acc| acc.created_at).unwrap_or(now);

    let mut account = existing.unwrap_or(CursorAccount {
        id: account_id.clone(),
        email: payload.email.clone(),
        auth_id: incoming_auth_id.clone(),
        name: payload.name.clone(),
        tags,
        session_notes,
        access_token: payload.access_token.clone(),
        refresh_token: payload.refresh_token.clone(),
        membership_type: payload.membership_type.clone(),
        subscription_status: payload.subscription_status.clone(),
        sign_up_type: payload.sign_up_type.clone(),
        cursor_auth_raw: payload.cursor_auth_raw.clone(),
        cursor_usage_raw: payload.cursor_usage_raw.clone(),
        cursor_credit_grants_raw: payload.cursor_credit_grants_raw.clone(),
        cursor_referral_raw: None,
        cursor_welcome_back_raw: None,
        cursor_free_credit_usage_raw: payload.cursor_free_credit_usage_raw.clone(),
        cursor_sand_usage_raw: None,
        cursor_last_usage_event_at: None,
        status: payload.status.clone(),
        status_reason: payload.status_reason.clone(),
        quota_query_last_error: None,
        quota_query_last_error_at: None,
        usage_updated_at: None,
        created_at,
        last_used: now,
    });

    apply_payload(&mut account, payload, incoming_auth_id);
    account.id = account_id;
    account.created_at = created_at;
    account.quota_query_last_error = None;
    account.quota_query_last_error_at = None;
    account.last_used = now;

    save_account_file(&account)?;
    refresh_summary(&mut index, &account);
    save_account_index(&index)?;

    logger::log_info(&format!(
        "Cursor 账号已保存: id={}, email={}",
        account.id, account.email
    ));
    Ok(account)
}

pub fn remove_account(account_id: &str) -> Result<(), String> {
    let _lock = CURSOR_ACCOUNT_INDEX_LOCK
        .lock()
        .map_err(|_| "获取 Cursor 账号锁失败".to_string())?;
    let mut index = load_account_index();
    index.accounts.retain(|item| item.id != account_id);
    save_account_index(&index)?;
    delete_account_file(account_id)?;
    Ok(())
}

pub fn remove_accounts(account_ids: &[String]) -> Result<(), String> {
    for id in account_ids {
        remove_account(id)?;
    }
    Ok(())
}

pub fn update_account_tags(account_id: &str, tags: Vec<String>) -> Result<CursorAccount, String> {
    let mut account = load_account(account_id).ok_or_else(|| "账号不存在".to_string())?;
    account.tags = Some(tags);
    account.last_used = now_ts();
    let updated = account.clone();
    upsert_account_record(account)?;
    Ok(updated)
}

pub fn update_session_note(
    account_id: &str,
    session_id: &str,
    note: String,
) -> Result<CursorAccount, String> {
    let mut account = load_account(account_id).ok_or_else(|| "账号不存在".to_string())?;
    let session_key = session_id.trim();
    if session_key.is_empty() {
        return Err("会话无效".to_string());
    }
    let trimmed = note
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let truncated: String = trimmed.chars().take(CURSOR_SESSION_NOTE_MAX_CHARS).collect();
    let mut notes = account.session_notes.take().unwrap_or_default();
    if truncated.is_empty() {
        notes.remove(session_key);
    } else {
        notes.insert(session_key.to_string(), truncated);
    }
    account.session_notes = if notes.is_empty() { None } else { Some(notes) };
    let updated = account.clone();
    upsert_account_record(account)?;
    Ok(updated)
}

// ---------------------------------------------------------------------------
// Import / Export
// ---------------------------------------------------------------------------

fn clone_object_value(value: Option<&Value>) -> Option<Value> {
    value.and_then(|raw| {
        if raw.is_object() {
            Some(raw.clone())
        } else {
            None
        }
    })
}

fn extract_string(obj: &serde_json::Map<String, Value>, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(value) = obj.get(*key) {
            if let Some(text) = value.as_str().map(str::trim).filter(|v| !v.is_empty()) {
                return Some(text.to_string());
            }
        }
    }
    None
}

fn normalize_exported_tags(tags: Vec<String>) -> Option<Vec<String>> {
    let cleaned: Vec<String> = tags
        .into_iter()
        .map(|tag| tag.trim().to_string())
        .filter(|tag| !tag.is_empty())
        .collect();
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned)
    }
}

fn extract_string_array(obj: &serde_json::Map<String, Value>, keys: &[&str]) -> Option<Vec<String>> {
    for key in keys {
        let Some(value) = obj.get(*key) else {
            continue;
        };
        if value.is_null() {
            return Some(Vec::new());
        }
        let Some(items) = value.as_array() else {
            continue;
        };
        return Some(
            items
                .iter()
                .filter_map(|item| {
                    item.as_str()
                        .map(str::trim)
                        .filter(|text| !text.is_empty())
                        .map(|text| text.to_string())
                })
                .collect(),
        );
    }
    None
}

fn payload_from_import_value(raw: Value) -> Result<CursorImportPayload, String> {
    let obj = raw
        .as_object()
        .ok_or_else(|| "Cursor 导入 JSON 必须是对象".to_string())?;

    let workos_token = extract_string(
        obj,
        &[
            "workos_cursor_session_token",
            "workosSessionToken",
            "workos_token",
            "workosToken",
        ],
    );
    let email = extract_string(obj, &["email", "cachedEmail", "cursor_email"])
        .unwrap_or_else(|| "unknown".to_string());
    let access_token = extract_string(
        obj,
        &[
            "access_token",
            "accessToken",
            "token",
            "cursor_access_token",
        ],
    )
    .or_else(|| {
        workos_token.as_ref().and_then(|token| {
            normalize_workos_session_token(token).and_then(|normalized| {
                normalized
                    .split_once("::")
                    .map(|(_, jwt)| jwt.to_string())
            })
        })
    })
    .ok_or_else(|| "缺少 access_token 或 workos_cursor_session_token 字段".to_string())?;

    let name = extract_string(obj, &["name", "displayName"]);
    let refresh_token = extract_string(
        obj,
        &["refresh_token", "refreshToken", "cursor_refresh_token"],
    );
    let membership_type = extract_string(
        obj,
        &[
            "membership_type",
            "membershipType",
            "stripeMembershipType",
            "plan",
        ],
    );
    let subscription_status = extract_string(
        obj,
        &[
            "subscription_status",
            "subscriptionStatus",
            "stripeSubscriptionStatus",
        ],
    );
    let sign_up_type = extract_string(obj, &["sign_up_type", "signUpType", "cachedSignUpType"]);
    let status = extract_string(obj, &["status"]);
    let status_reason = extract_string(obj, &["status_reason", "statusReason"]);

    let mut cursor_auth_raw = clone_object_value(obj.get("cursor_auth_raw"))
        .or_else(|| clone_object_value(obj.get("cursorAuthRaw")));
    if let Some(workos_token) = workos_token.as_ref() {
        if let Some(normalized) = normalize_workos_session_token(workos_token) {
            let auth_map = match cursor_auth_raw.as_mut() {
                Some(Value::Object(map)) => map,
                _ => {
                    cursor_auth_raw = Some(Value::Object(serde_json::Map::new()));
                    cursor_auth_raw
                        .as_mut()
                        .and_then(|value| value.as_object_mut())
                        .expect("cursor_auth_raw 应为对象")
                }
            };
            auth_map.insert(
                "workosSessionToken".to_string(),
                Value::String(normalized),
            );
        }
    }
    let cursor_usage_raw = clone_object_value(obj.get("cursor_usage_raw"))
        .or_else(|| clone_object_value(obj.get("cursorUsageRaw")));
    let cursor_credit_grants_raw = clone_object_value(obj.get("cursor_credit_grants_raw"))
        .or_else(|| clone_object_value(obj.get("cursorCreditGrantsRaw")));
    let cursor_free_credit_usage_raw = clone_object_value(obj.get("cursor_free_credit_usage_raw"))
        .or_else(|| clone_object_value(obj.get("cursorFreeCreditUsageRaw")));
    let auth_id = extract_string(obj, &["auth_id", "authId", "workos_id", "workosId"])
        .or_else(|| extract_auth_id_from_raw_value(cursor_auth_raw.as_ref()))
        .or_else(|| extract_auth_id_from_access_token(access_token.as_str()));
    let tags = extract_string_array(obj, &["tags"]);

    Ok(CursorImportPayload {
        email,
        auth_id,
        name,
        access_token,
        refresh_token,
        membership_type,
        subscription_status,
        sign_up_type,
        cursor_auth_raw,
        cursor_usage_raw,
        cursor_credit_grants_raw,
        cursor_free_credit_usage_raw,
        status,
        status_reason,
        tags,
    })
}

fn payloads_from_import_json_value(value: Value) -> Result<Vec<CursorImportPayload>, String> {
    match value {
        Value::Array(items) => {
            if items.is_empty() {
                return Err("导入数组为空".to_string());
            }
            let mut payloads = Vec::with_capacity(items.len());
            for (idx, item) in items.into_iter().enumerate() {
                let payload = payload_from_import_value(item)
                    .map_err(|e| format!("第 {} 条 Cursor 账号解析失败: {}", idx + 1, e))?;
                payloads.push(payload);
            }
            Ok(payloads)
        }
        Value::Object(mut obj) => {
            let object_value = Value::Object(obj.clone());
            if let Ok(payload) = payload_from_import_value(object_value) {
                return Ok(vec![payload]);
            }

            if let Some(accounts) = obj
                .remove("accounts")
                .or_else(|| obj.remove("items"))
                .and_then(|raw| raw.as_array().cloned())
            {
                if accounts.is_empty() {
                    return Err("导入数组为空".to_string());
                }
                let mut payloads = Vec::with_capacity(accounts.len());
                for (idx, item) in accounts.into_iter().enumerate() {
                    let payload = payload_from_import_value(item)
                        .map_err(|e| format!("第 {} 条 Cursor 账号解析失败: {}", idx + 1, e))?;
                    payloads.push(payload);
                }
                return Ok(payloads);
            }

            Err("无法解析 Cursor 导入对象".to_string())
        }
        _ => Err("Cursor 导入 JSON 必须是对象或数组".to_string()),
    }
}

pub fn import_from_json(json_content: &str) -> Result<Vec<CursorAccount>, String> {
    if let Ok(account) = serde_json::from_str::<CursorAccount>(json_content) {
        let saved = upsert_account_record(account)?;
        return Ok(vec![saved]);
    }

    if let Ok(accounts) = serde_json::from_str::<Vec<CursorAccount>>(json_content) {
        let mut result = Vec::new();
        for account in accounts {
            let saved = upsert_account_record(account)?;
            result.push(saved);
        }
        return Ok(result);
    }

    if let Ok(value) = serde_json::from_str::<Value>(json_content) {
        if let Ok(payloads) = payloads_from_import_json_value(value) {
            let mut result = Vec::with_capacity(payloads.len());
            for payload in payloads {
                let saved = upsert_account(payload)?;
                result.push(saved);
            }
            return Ok(result);
        }
    }

    Err("无法解析 JSON 内容".to_string())
}

pub fn export_accounts(account_ids: &[String]) -> Result<String, String> {
    let accounts: Vec<CursorAccount> = account_ids
        .iter()
        .filter_map(|id| load_account(id))
        .collect();

    let export_items: Vec<Value> = accounts
        .into_iter()
        .map(build_cursor_export_item)
        .collect();

    if export_items.is_empty() {
        return Err("未找到可导出的账号".to_string());
    }

    let payload = if export_items.len() == 1 {
        export_items.into_iter().next().unwrap_or(Value::Null)
    } else {
        Value::Array(export_items)
    };

    serde_json::to_string_pretty(&payload).map_err(|e| format!("序列化失败: {}", e))
}

fn build_cursor_export_item(account: CursorAccount) -> Value {
    let mut obj = serde_json::Map::new();
    let email = account.email.trim();
    if !email.is_empty() && !email.eq_ignore_ascii_case("unknown") {
        obj.insert("email".to_string(), Value::String(email.to_string()));
    }
    obj.insert(
        "token".to_string(),
        Value::String(account.access_token.clone()),
    );
    obj.insert(
        "access_token".to_string(),
        Value::String(account.access_token.clone()),
    );
    if let Some(refresh_token) = normalize_non_empty(account.refresh_token.as_deref()) {
        obj.insert("refresh_token".to_string(), Value::String(refresh_token));
    }
    if let Some(workos_token) = read_workos_session_token(&account) {
        obj.insert(
            "workos_cursor_session_token".to_string(),
            Value::String(workos_token),
        );
    }
    if let Some(membership_type) = normalize_non_empty(account.membership_type.as_deref()) {
        obj.insert(
            "membership_type".to_string(),
            Value::String(membership_type),
        );
    }
    obj.insert(
        "tags".to_string(),
        Value::Array(
            account
                .tags
                .unwrap_or_default()
                .into_iter()
                .map(Value::String)
                .collect(),
        ),
    );
    Value::Object(obj)
}

fn cursor_dashboard_cursor_host(host: &str) -> bool {
    host == "cursor.com"
        || host.ends_with(".cursor.com")
        || host == "cursor.sh"
        || host.ends_with(".cursor.sh")
}

/// Stripe.js / 反欺诈 SDK 在 Dashboard 内创建的 iframe 基础设施域名。
fn cursor_dashboard_embed_infrastructure_host(host: &str) -> bool {
    host == "js.stripe.com"
        || host == "m.stripe.com"
        || host == "m.stripe.network"
        || host.ends_with(".stripe.network")
        || host == "b.stripecdn.com"
        || host.ends_with(".stripecdn.com")
        || host == "hcaptcha.com"
        || host.ends_with(".hcaptcha.com")
        || host == "px-cloud.net"
        || host.ends_with(".px-cloud.net")
}

/// 用户主动点击后应在系统浏览器打开的 Stripe 账单/支付页。
fn cursor_dashboard_should_open_in_browser_host(host: &str) -> bool {
    host == "billing.stripe.com"
        || host == "checkout.stripe.com"
        || host == "invoice.stripe.com"
        || host == "pay.stripe.com"
        || ((host.starts_with("billing.") || host.starts_with("checkout."))
            && host.ends_with(".stripe.com"))
}

fn cursor_dashboard_host_allowed(host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    cursor_dashboard_cursor_host(&host) || cursor_dashboard_embed_infrastructure_host(&host)
}

enum CursorDashboardNavigationAction {
    AllowInWebview,
    OpenInBrowser,
    DenySilently,
}

fn cursor_dashboard_navigation_action(url: &url::Url) -> CursorDashboardNavigationAction {
    match url.scheme() {
        "about" if url.path() == "blank" => CursorDashboardNavigationAction::AllowInWebview,
        "https" | "http" => {
            let Some(host) = url.host_str() else {
                return CursorDashboardNavigationAction::DenySilently;
            };
            let host = host.trim_end_matches('.').to_ascii_lowercase();
            if cursor_dashboard_host_allowed(&host) {
                CursorDashboardNavigationAction::AllowInWebview
            } else if cursor_dashboard_should_open_in_browser_host(&host) {
                CursorDashboardNavigationAction::OpenInBrowser
            } else {
                CursorDashboardNavigationAction::DenySilently
            }
        }
        _ => CursorDashboardNavigationAction::DenySilently,
    }
}

fn cursor_dashboard_url_stays_in_webview(url: &url::Url) -> bool {
    matches!(
        cursor_dashboard_navigation_action(url),
        CursorDashboardNavigationAction::AllowInWebview
    )
}

#[derive(Default)]
struct CursorDashboardExternalOpenDeduper {
    last_url: Option<String>,
    last_at: Option<Instant>,
}

impl CursorDashboardExternalOpenDeduper {
    fn should_open(&mut self, url: &url::Url) -> bool {
        let key = url.as_str();
        let now = Instant::now();
        if let (Some(last_url), Some(last_at)) = (&self.last_url, self.last_at) {
            if last_url == key && now.duration_since(last_at) < Duration::from_millis(800) {
                logger::log_info(&format!(
                    "[Cursor Dashboard] 跳过重复的外部链接打开: {}",
                    url
                ));
                return false;
            }
        }
        self.last_url = Some(key.to_string());
        self.last_at = Some(now);
        true
    }
}

fn cursor_dashboard_open_external(
    app: &tauri::AppHandle,
    url: &url::Url,
    deduper: &Arc<Mutex<CursorDashboardExternalOpenDeduper>>,
) {
    use tauri_plugin_opener::OpenerExt;

    {
        let mut guard = deduper
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !guard.should_open(url) {
            return;
        }
    }

    logger::log_info(&format!(
        "[Cursor Dashboard] 在系统浏览器打开外部链接: {}",
        url
    ));
    if let Err(err) = app.opener().open_url(url.as_str(), None::<String>) {
        logger::log_warn(&format!(
            "[Cursor Dashboard] 打开外部链接失败: url={}, error={}",
            url, err
        ));
    }
}

#[cfg(target_os = "linux")]
fn cursor_dashboard_linux_scroll_wheel_fix_script() -> &'static str {
    r#"
(function installCursorDashboardScrollWheelFix() {
  if (window.__cockpitCursorDashboardScrollFix) return;
  window.__cockpitCursorDashboardScrollFix = true;

  function normalizeWheelDelta(event) {
    let delta = event.deltaY;
    if (event.deltaMode === 1) delta *= 16;
    else if (event.deltaMode === 2) delta *= window.innerHeight;
    return delta;
  }

  function canScrollVertically(el) {
    if (!(el instanceof Element)) return false;
    const style = window.getComputedStyle(el);
    const overflowY = style.overflowY;
    if (overflowY !== 'auto' && overflowY !== 'scroll' && overflowY !== 'overlay') {
      return false;
    }
    return el.scrollHeight > el.clientHeight + 1;
  }

  function findScrollTarget(start, delta) {
    let el = start instanceof Element ? start : null;
    while (el) {
      if (canScrollVertically(el)) {
        const maxScroll = el.scrollHeight - el.clientHeight;
        const next = el.scrollTop + delta;
        if (next > 0 && next < maxScroll) return el;
        if ((delta < 0 && el.scrollTop > 0) || (delta > 0 && el.scrollTop < maxScroll)) {
          return el;
        }
      }
      if (el === document.body || el === document.documentElement) break;
      el = el.parentElement;
    }
    return document.scrollingElement || document.documentElement;
  }

  function onWheel(event) {
    if (event.defaultPrevented || event.ctrlKey) return;
    const delta = normalizeWheelDelta(event);
    if (!delta) return;

    const target = findScrollTarget(event.target, delta);
    if (!target) return;

    const before = target.scrollTop;
    target.scrollTop += delta;
    if (target.scrollTop !== before) {
      event.preventDefault();
    }
  }

  window.addEventListener('wheel', onWheel, { capture: true, passive: false });
  document.addEventListener('wheel', onWheel, { capture: true, passive: false });
})();
"#
}

#[cfg(not(target_os = "linux"))]
fn cursor_dashboard_linux_scroll_wheel_fix_script() -> &'static str {
    ""
}

fn cursor_dashboard_stripe_fallback_script() -> &'static str {
    r#"
(function installCursorDashboardStripeFallback() {
  const STRIPE_BUTTON_PATTERN = /manage in stripe|manage subscription|stripe billing|管理 stripe|stripe 账单/i;

  function toSessionCookieValue(token) {
    if (token.includes('::')) {
      const idx = token.indexOf('::');
      return token.slice(0, idx) + '%3A%3A' + token.slice(idx + 2);
    }
    return token;
  }

  function setSessionCookies(token) {
    const secure = location.protocol === 'https:' ? '; Secure' : '';
    const sessionValue = toSessionCookieValue(token);
    document.cookie = 'generaltranslation.locale-routing-enabled=true; domain=.cursor.com; path=/; SameSite=Lax' + secure;
    document.cookie = 'NEXT_LOCALE=cn; domain=.cursor.com; path=/; SameSite=Lax' + secure;
    document.cookie = 'WorkosCursorSessionToken=' + sessionValue + '; domain=.cursor.com; path=/; SameSite=Lax' + secure;
  }

  function normalizePortalUrl(raw) {
    return String(raw || '').trim().replace(/^"+|"+$/g, '');
  }

  async function openStripePortalFromApi() {
    const res = await fetch('https://cursor.com/api/stripeSession', {
      method: 'GET',
      credentials: 'include',
      headers: { Accept: '*/*' },
    });
    if (!res.ok) {
      throw new Error('stripeSession HTTP ' + res.status);
    }
    const url = normalizePortalUrl(await res.text());
    if (!url.startsWith('http')) {
      throw new Error('invalid stripe portal url');
    }
    window.location.assign(url);
  }

  function installStripeClickFallback(token) {
    if (window.__cockpitCursorStripeClickFallbackInstalled) return;
    window.__cockpitCursorStripeClickFallbackInstalled = true;
    document.addEventListener('click', function (event) {
      const el = event.target && event.target.closest
        ? event.target.closest('button, a, [role="button"]')
        : null;
      if (!el) return;
      const text = (el.textContent || '').trim();
      if (!STRIPE_BUTTON_PATTERN.test(text)) return;
      event.preventDefault();
      event.stopImmediatePropagation();
      setSessionCookies(token);
      openStripePortalFromApi().catch(function (err) {
        console.error('[cockpit-tools] Stripe portal failed', err);
        alert('无法打开 Stripe 账单页: ' + err);
      });
    }, true);
  }

  window.__cockpitCursorDashboardOpenStripe = openStripePortalFromApi;
  return { setSessionCookies, installStripeClickFallback };
})();
"#
}

fn build_cursor_dashboard_init_script(workos_token: &str) -> String {
    let token_json =
        serde_json::to_string(workos_token).unwrap_or_else(|_| "\"\"".to_string());
    format!(
        r#"(function() {{
  try {{
    const token = {token_json};
    const helpers = {stripe_helpers};
    helpers.setSessionCookies(token);
    helpers.installStripeClickFallback(token);
    if (!window.location.pathname.includes('/dashboard')) {{
      window.location.replace('https://cursor.com/cn/dashboard');
    }}
  }} catch (error) {{
    console.error('[cockpit-tools] Failed to inject Cursor session cookie', error);
  }}
}})();
{scroll_fix}"#,
        token_json = token_json,
        stripe_helpers = cursor_dashboard_stripe_fallback_script().trim(),
        scroll_fix = cursor_dashboard_linux_scroll_wheel_fix_script()
    )
}

pub async fn open_cursor_dashboard(
    app: &tauri::AppHandle,
    account_id: &str,
) -> Result<(), String> {
    open_cursor_authenticated_window(app, account_id, "https://cursor.com/cn").await
}

async fn open_cursor_authenticated_window(
    app: &tauri::AppHandle,
    account_id: &str,
    start_url: &str,
) -> Result<(), String> {
    use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

    let account = load_account(account_id).ok_or_else(|| "账号不存在".to_string())?;
    let workos_token = read_workos_session_token(&account)
        .ok_or_else(|| "无法解析 WorkOS Session Token，请重新导入账号".to_string())?;

    logger::log_info(&format!(
        "[Cursor Dashboard] 打开窗口: account_id={}, email={}",
        account.id, account.email
    ));

    if let Some(existing_window) = app.get_webview_window("cursor_dashboard") {
        let _ = existing_window.close();
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    let init_script = build_cursor_dashboard_init_script(&workos_token);
    let external_open_deduper = Arc::new(Mutex::new(CursorDashboardExternalOpenDeduper::default()));
    let app_for_navigation = app.clone();
    let app_for_new_window = app.clone();
    let deduper_for_navigation = Arc::clone(&external_open_deduper);
    let deduper_for_new_window = Arc::clone(&external_open_deduper);

    let window = WebviewWindowBuilder::new(
        app,
        "cursor_dashboard",
        WebviewUrl::External(
            start_url
                .parse()
                .map_err(|e| format!("无效的 Dashboard URL: {}", e))?,
        ),
    )
    .title("Cursor Dashboard")
    .inner_size(1200.0, 800.0)
    .resizable(true)
    .user_agent("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/148.0.0.0 Safari/537.36")
    .initialization_script(&init_script)
    .on_navigation(move |url| {
        match cursor_dashboard_navigation_action(url) {
            CursorDashboardNavigationAction::AllowInWebview => true,
            CursorDashboardNavigationAction::OpenInBrowser => {
                cursor_dashboard_open_external(&app_for_navigation, url, &deduper_for_navigation);
                false
            }
            CursorDashboardNavigationAction::DenySilently => false,
        }
    })
    .on_new_window(move |url, _features| {
        match cursor_dashboard_navigation_action(&url) {
            CursorDashboardNavigationAction::AllowInWebview => {
                tauri::webview::NewWindowResponse::Allow
            }
            CursorDashboardNavigationAction::OpenInBrowser => {
                cursor_dashboard_open_external(&app_for_new_window, &url, &deduper_for_new_window);
                tauri::webview::NewWindowResponse::Deny
            }
            CursorDashboardNavigationAction::DenySilently => {
                tauri::webview::NewWindowResponse::Deny
            }
        }
    })
    .build()
    .map_err(|e| format!("打开 Cursor 主页失败: {}", e))?;

    let _ = window.set_focus();

    Ok(())
}

async fn fetch_stripe_billing_portal_url(account: &CursorAccount) -> Result<String, String> {
    let client = build_cursor_http_client()?;
    let cookie = account_dashboard_cookie(account)?;
    let response = client
        .get(CURSOR_STRIPE_SESSION_URL)
        .header("Accept", "*/*")
        .header("Cookie", &cookie)
        .header("Origin", "https://cursor.com")
        .header("Referer", "https://cursor.com/dashboard")
        .header(
            "User-Agent",
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/148.0.0.0 Safari/537.36",
        )
        .timeout(std::time::Duration::from_secs(40))
        .send()
        .await
        .map_err(|e| format!("请求 Stripe 账单入口失败: {}", e))?;

    let status = response.status().as_u16();
    if status == 401 || status == 403 {
        return Err("Cursor 会话已过期或未认证，请重新导入账号".to_string());
    }
    if !response.status().is_success() {
        return Err(format!("Stripe 账单入口返回异常状态码: {}", status));
    }

    let body = response
        .text()
        .await
        .map_err(|e| format!("读取 Stripe 账单入口响应失败: {}", e))?;
    let url = body.trim().trim_matches('"').to_string();
    if !url.starts_with("http") {
        return Err(format!("Stripe 账单入口 URL 无效: {}", url));
    }
    Ok(url)
}

pub async fn open_cursor_stripe_billing(
    app: &tauri::AppHandle,
    account_id: &str,
) -> Result<(), String> {
    let account = load_account(account_id).ok_or_else(|| "账号不存在".to_string())?;
    logger::log_info(&format!(
        "[Cursor Dashboard] 请求 Stripe 账单入口: account_id={}, email={}",
        account.id, account.email
    ));
    let url_str = fetch_stripe_billing_portal_url(&account).await?;
    logger::log_info(&format!(
        "[Cursor Dashboard] Stripe 账单入口获取成功: account_id={}",
        account.id
    ));
    let url = url_str
        .parse::<url::Url>()
        .map_err(|e| format!("Stripe 账单入口 URL 解析失败: {}", e))?;
    cursor_dashboard_open_external(
        app,
        &url,
        &Arc::new(Mutex::new(CursorDashboardExternalOpenDeduper::default())),
    );
    Ok(())
}

fn is_free_cursor_membership(membership_type: Option<&str>) -> bool {
    normalize_non_empty(membership_type)
        .map(|value| value.eq_ignore_ascii_case("free"))
        .unwrap_or(false)
}

fn welcome_back_can_activate(value: &serde_json::Value) -> bool {
    value
        .get("canActivate")
        .or_else(|| value.get("can_activate"))
        .and_then(|item| item.as_bool())
        == Some(true)
}

fn extract_http_url(value: &str) -> Option<String> {
    let trimmed = value.trim().trim_matches('"').trim();
    if trimmed.starts_with("https://") || trimmed.starts_with("http://") {
        Some(trimmed.to_string())
    } else {
        None
    }
}

fn extract_checkout_url_from_json(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(text) => extract_http_url(text),
        serde_json::Value::Object(map) => {
            for key in [
                "url",
                "checkoutUrl",
                "checkout_url",
                "sessionUrl",
                "session_url",
                "redirectUrl",
                "redirect_url",
                "paymentUrl",
                "payment_url",
                "href",
            ] {
                if let Some(found) = map.get(key).and_then(extract_checkout_url_from_json) {
                    return Some(found);
                }
            }
            for key in ["checkout", "data", "result", "payload"] {
                if let Some(found) = map.get(key).and_then(extract_checkout_url_from_json) {
                    return Some(found);
                }
            }
            None
        }
        _ => None,
    }
}

fn is_preferred_checkout_url(url: &str) -> bool {
    let Ok(parsed) = url::Url::parse(url) else {
        return false;
    };
    let host = parsed.host_str().unwrap_or("").to_ascii_lowercase();
    cursor_dashboard_should_open_in_browser_host(&host)
}

fn is_cursor_api_welcome_back_url(url: &str) -> bool {
    url.contains("cursor.com/api/auth/welcome-back")
}

fn is_cursor_web_welcome_back_url(url: &str) -> bool {
    let Ok(parsed) = url::Url::parse(url) else {
        return false;
    };
    let host = parsed.host_str().unwrap_or("").to_ascii_lowercase();
    if !cursor_dashboard_cursor_host(&host) || is_cursor_api_welcome_back_url(url) {
        return false;
    }
    let path = parsed.path().to_ascii_lowercase();
    path.contains("welcome-back") || path.contains("checkout")
}

fn is_allowed_welcome_back_checkout_url(url: &str) -> bool {
    is_preferred_checkout_url(url) || is_cursor_web_welcome_back_url(url)
}

fn resolve_welcome_back_checkout_url(final_url: &str, body: &str) -> Result<String, String> {
    if is_preferred_checkout_url(final_url) {
        return Ok(final_url.to_string());
    }
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(body) {
        if let Some(url) = extract_checkout_url_from_json(&json) {
            if is_allowed_welcome_back_checkout_url(&url) {
                return Ok(url);
            }
        }
    }
    if let Some(url) = extract_http_url(body) {
        if is_allowed_welcome_back_checkout_url(&url) {
            return Ok(url);
        }
    }
    if is_cursor_web_welcome_back_url(final_url) {
        return Ok(final_url.to_string());
    }
    Err("未获取到 Comeback 支付链接".to_string())
}

async fn get_cursor_web_json_with_client(
    client: &reqwest::Client,
    cookie: &str,
    url: &str,
    referer: &str,
) -> Result<serde_json::Value, String> {
    let response = client
        .get(url)
        .header("Accept", "*/*")
        .header("Cookie", cookie)
        .header("Origin", "https://cursor.com")
        .header("Referer", referer)
        .header(
            "User-Agent",
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/148.0.0.0 Safari/537.36",
        )
        .send()
        .await
        .map_err(|e| format!("请求 Cursor 接口失败: {}", e))?;

    let status = response.status().as_u16();
    if status == 401 || status == 403 {
        return Err(format!(
            "Cursor 会话已过期或未认证，请重新导入账号 (HTTP {})",
            status
        ));
    }
    if status != 200 {
        return Err(format!("Cursor 接口返回异常状态码: {}", status));
    }

    let body = response
        .text()
        .await
        .map_err(|e| format!("读取 Cursor 接口响应失败: {}", e))?;
    serde_json::from_str::<serde_json::Value>(&body)
        .map_err(|e| format!("解析 Cursor 接口 JSON 失败: {}", e))
}

async fn fetch_welcome_back_offer_with_client(
    client: &reqwest::Client,
    account: &CursorAccount,
) -> Result<serde_json::Value, String> {
    let cookie = account_dashboard_cookie(account)?;
    match get_cursor_web_json_with_client(
        client,
        &cookie,
        CURSOR_WELCOME_BACK_OFFER_URL,
        CURSOR_WELCOME_BACK_REFERER,
    )
    .await
    {
        Ok(response) => Ok(response),
        Err(err) if is_cursor_session_error(&err) => Err(err),
        Err(err) if err.contains("403") || err.contains("404") => {
            Ok(serde_json::json!({ "canActivate": false }))
        }
        Err(err) => Err(err),
    }
}

async fn fetch_welcome_back_checkout_url(account: &CursorAccount) -> Result<String, String> {
    let client = build_cursor_http_client()?;
    let cookie = account_dashboard_cookie(account)?;
    let response = client
        .get(CURSOR_WELCOME_BACK_CHECKOUT_URL)
        .header("Accept", "*/*")
        .header("Cookie", &cookie)
        .header("Origin", "https://cursor.com")
        .header("Referer", CURSOR_WELCOME_BACK_REFERER)
        .header(
            "User-Agent",
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/148.0.0.0 Safari/537.36",
        )
        .timeout(std::time::Duration::from_secs(40))
        .send()
        .await
        .map_err(|e| format!("请求 Comeback 支付链接失败: {}", e))?;

    let status = response.status().as_u16();
    if status == 401 || status == 403 {
        return Err("Cursor 会话已过期或未认证，请重新导入账号".to_string());
    }

    let final_url = response.url().to_string();
    let body = response
        .text()
        .await
        .map_err(|e| format!("读取 Comeback 支付链接响应失败: {}", e))?;

    if status != 200 && status != 201 && !is_preferred_checkout_url(&final_url) {
        if let Ok(url) = resolve_welcome_back_checkout_url(&final_url, &body) {
            return Ok(url);
        }
        return Err(format!("Comeback 支付接口返回异常状态码: {}", status));
    }

    resolve_welcome_back_checkout_url(&final_url, &body)
}

pub async fn fetch_cursor_welcome_back_checkout(
    app: &tauri::AppHandle,
    account_id: &str,
) -> Result<String, String> {
    let account = load_account(account_id).ok_or_else(|| "账号不存在".to_string())?;
    if !is_free_cursor_membership(account.membership_type.as_deref()) {
        return Err("Comeback 优惠仅对 FREE 账号有效".to_string());
    }

    logger::log_info(&format!(
        "[Cursor WelcomeBack] 请求支付链接: account_id={}, email={}",
        account.id, account.email
    ));
    let url_str = fetch_welcome_back_checkout_url(&account).await?;
    if !is_allowed_welcome_back_checkout_url(&url_str) {
        return Err("Comeback 支付链接无效".to_string());
    }
    logger::log_info(&format!(
        "[Cursor WelcomeBack] 支付链接获取成功: account_id={}",
        account.id
    ));

    let url = url_str
        .parse::<url::Url>()
        .map_err(|e| format!("Comeback 支付链接解析失败: {}", e))?;
    let host = url.host_str().unwrap_or("").to_ascii_lowercase();
    if cursor_dashboard_cursor_host(&host) && !is_preferred_checkout_url(&url_str) {
        open_cursor_authenticated_window(app, account_id, &url_str).await?;
    } else {
        cursor_dashboard_open_external(
            app,
            &url,
            &Arc::new(Mutex::new(CursorDashboardExternalOpenDeduper::default())),
        );
    }
    Ok(url_str)
}

// ---------------------------------------------------------------------------
// Local import (read from Cursor's state.vscdb)
// ---------------------------------------------------------------------------

pub fn get_default_cursor_data_dir() -> Result<PathBuf, String> {
    #[cfg(target_os = "macos")]
    {
        let home = dirs::home_dir().ok_or("无法获取用户主目录")?;
        return Ok(home.join("Library/Application Support/Cursor"));
    }

    #[cfg(target_os = "windows")]
    {
        let appdata =
            std::env::var("APPDATA").map_err(|_| "无法获取 APPDATA 环境变量".to_string())?;
        return Ok(PathBuf::from(appdata).join("Cursor"));
    }

    #[cfg(target_os = "linux")]
    {
        let home = dirs::home_dir().ok_or("无法获取用户主目录")?;
        return Ok(home.join(".config/Cursor"));
    }

    #[allow(unreachable_code)]
    Err("Cursor 账号导入仅支持 macOS、Windows 和 Linux".to_string())
}

pub fn get_default_cursor_state_db_path() -> Result<PathBuf, String> {
    Ok(get_default_cursor_data_dir()?
        .join("User")
        .join("globalStorage")
        .join("state.vscdb"))
}

fn read_vscdb_item(conn: &Connection, key: &str) -> Option<String> {
    conn.query_row("SELECT value FROM ItemTable WHERE key = ?1", [key], |row| {
        row.get::<_, String>(0)
    })
    .optional()
    .ok()
    .flatten()
    .and_then(|v| {
        let trimmed = v.trim().to_string();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    })
}

pub fn read_local_cursor_auth() -> Result<Option<CursorImportPayload>, String> {
    let db_path = get_default_cursor_state_db_path()?;
    if !db_path.exists() {
        return Ok(None);
    }

    let conn = Connection::open(&db_path)
        .map_err(|e| format!("打开 Cursor 本地数据库失败({}): {}", db_path.display(), e))?;

    let access_token = match read_vscdb_item(&conn, "cursorAuth/accessToken") {
        Some(t) => t,
        None => return Ok(None),
    };

    let email = read_vscdb_item(&conn, "cursorAuth/cachedEmail").unwrap_or_default();
    if email.is_empty() {
        return Ok(None);
    }

    let refresh_token = read_vscdb_item(&conn, "cursorAuth/refreshToken");
    let auth_id = read_vscdb_item(&conn, "cursorAuth/authId")
        .or_else(|| extract_auth_id_from_access_token(access_token.as_str()));
    let membership_type = read_vscdb_item(&conn, "cursorAuth/stripeMembershipType");
    let subscription_status = read_vscdb_item(&conn, "cursorAuth/stripeSubscriptionStatus");
    let sign_up_type = read_vscdb_item(&conn, "cursorAuth/cachedSignUpType");

    let mut auth_raw = serde_json::Map::new();
    auth_raw.insert(
        "accessToken".to_string(),
        Value::String(access_token.clone()),
    );
    if let Some(ref rt) = refresh_token {
        auth_raw.insert("refreshToken".to_string(), Value::String(rt.clone()));
    }
    if let Some(ref auth_id_value) = auth_id {
        auth_raw.insert("authId".to_string(), Value::String(auth_id_value.clone()));
    }
    auth_raw.insert("cachedEmail".to_string(), Value::String(email.clone()));
    if let Some(ref mt) = membership_type {
        auth_raw.insert(
            "stripeMembershipType".to_string(),
            Value::String(mt.clone()),
        );
    }
    if let Some(ref ss) = subscription_status {
        auth_raw.insert(
            "stripeSubscriptionStatus".to_string(),
            Value::String(ss.clone()),
        );
    }
    if let Some(ref st) = sign_up_type {
        auth_raw.insert("cachedSignUpType".to_string(), Value::String(st.clone()));
    }

    Ok(Some(CursorImportPayload {
        email,
        auth_id,
        name: None,
        access_token,
        refresh_token,
        membership_type,
        subscription_status,
        sign_up_type,
        cursor_auth_raw: Some(Value::Object(auth_raw)),
        cursor_usage_raw: None,
        cursor_credit_grants_raw: None,
        cursor_free_credit_usage_raw: None,
        status: None,
        status_reason: None,
        tags: None,
    }))
}

pub fn import_from_local() -> Result<Option<CursorAccount>, String> {
    let payload = match read_local_cursor_auth()? {
        Some(p) => p,
        None => return Ok(None),
    };
    let account = upsert_account(payload)?;
    logger::log_info(&format!(
        "[Cursor Account] 从本地导入成功: id={}, email={}",
        account.id, account.email
    ));
    Ok(Some(account))
}

// ---------------------------------------------------------------------------
// Inject (write auth fields back to Cursor's state.vscdb)
// ---------------------------------------------------------------------------

fn upsert_vscdb_item(conn: &Connection, key: &str, value: &str) -> Result<(), String> {
    conn.execute(
        "INSERT OR REPLACE INTO ItemTable (key, value) VALUES (?1, ?2)",
        (key, value),
    )
    .map_err(|e| format!("写入 {} 失败: {}", key, e))?;
    Ok(())
}

pub fn inject_to_cursor(account_id: &str) -> Result<(), String> {
    let account =
        load_account(account_id).ok_or_else(|| format!("Cursor 账号不存在: {}", account_id))?;
    let db_path = get_default_cursor_state_db_path()?;
    if !db_path.exists() {
        return Err(format!("Cursor state.vscdb 不存在: {}", db_path.display()));
    }

    let conn =
        Connection::open(&db_path).map_err(|e| format!("打开 Cursor 本地数据库失败: {}", e))?;

    upsert_vscdb_item(&conn, "cursorAuth/accessToken", &account.access_token)?;
    if let Some(ref rt) = account.refresh_token {
        upsert_vscdb_item(&conn, "cursorAuth/refreshToken", rt)?;
    }
    upsert_vscdb_item(&conn, "cursorAuth/cachedEmail", &account.email)?;
    if let Some(ref mt) = account.membership_type {
        upsert_vscdb_item(&conn, "cursorAuth/stripeMembershipType", mt)?;
    }
    if let Some(ref ss) = account.subscription_status {
        upsert_vscdb_item(&conn, "cursorAuth/stripeSubscriptionStatus", ss)?;
    }

    upsert_vscdb_item(&conn, "cursor.accessToken", &account.access_token)?;
    upsert_vscdb_item(&conn, "cursor.email", &account.email)?;

    logger::log_info(&format!(
        "[Cursor Account] 注入成功: id={}, email={}",
        account.id, account.email
    ));
    Ok(())
}

pub fn inject_to_cursor_at_path(db_path: &std::path::Path, account_id: &str) -> Result<(), String> {
    let account =
        load_account(account_id).ok_or_else(|| format!("Cursor 账号不存在: {}", account_id))?;
    if !db_path.exists() {
        return Err(format!("Cursor state.vscdb 不存在: {}", db_path.display()));
    }

    let conn =
        Connection::open(db_path).map_err(|e| format!("打开 Cursor 本地数据库失败: {}", e))?;

    upsert_vscdb_item(&conn, "cursorAuth/accessToken", &account.access_token)?;
    if let Some(ref rt) = account.refresh_token {
        upsert_vscdb_item(&conn, "cursorAuth/refreshToken", rt)?;
    }
    upsert_vscdb_item(&conn, "cursorAuth/cachedEmail", &account.email)?;
    if let Some(ref mt) = account.membership_type {
        upsert_vscdb_item(&conn, "cursorAuth/stripeMembershipType", mt)?;
    }
    if let Some(ref ss) = account.subscription_status {
        upsert_vscdb_item(&conn, "cursorAuth/stripeSubscriptionStatus", ss)?;
    }

    upsert_vscdb_item(&conn, "cursor.accessToken", &account.access_token)?;
    upsert_vscdb_item(&conn, "cursor.email", &account.email)?;

    logger::log_info(&format!(
        "[Cursor Account] 注入成功(自定义路径): id={}, email={}, path={}",
        account.id,
        account.email,
        db_path.display()
    ));
    Ok(())
}

// ---------------------------------------------------------------------------
// Cursor usage API
// ---------------------------------------------------------------------------

const CURSOR_USAGE_SUMMARY_URL: &str = "https://cursor.com/api/usage-summary";
const CURSOR_CREDIT_GRANTS_BALANCE_URL: &str =
    "https://cursor.com/api/dashboard/get-credit-grants-balance";
const CURSOR_CLIENT_VISIBLE_CREDIT_GRANTS_URL: &str =
    "https://cursor.com/api/dashboard/get-client-visible-credit-grants";
const CURSOR_WELCOME_BACK_OFFER_URL: &str = "https://cursor.com/api/auth/welcome-back-offer";
const CURSOR_WELCOME_BACK_CHECKOUT_URL: &str = "https://cursor.com/api/auth/welcome-back?checkout=1";
const CURSOR_WELCOME_BACK_REFERER: &str = "https://cursor.com/activate/welcome-back/offer";
const CURSOR_P2P_REFERRAL_STATUS_URL: &str =
    "https://cursor.com/api/dashboard/get-p2p-referral-status";
const CURSOR_SAND_USAGE_STATUS_URL: &str =
    "https://cursor.com/api/dashboard/get-sand-usage-status";
const CURSOR_SAND_ACCESS_STATUS_URL: &str =
    "https://cursor.com/api/dashboard/get-sand-access-status";
const CURSOR_GET_USER_META_URL: &str = "https://api2.cursor.sh/aiserver.v1.AuthService/GetUserMeta";
const CURSOR_FULL_STRIPE_PROFILE_URL: &str = "https://api2.cursor.sh/auth/full_stripe_profile";
const CURSOR_STRIPE_PROFILE_URL: &str = "https://api2.cursor.sh/auth/stripe_profile";
// 与官方 Cursor 客户端保持一致：使用 api2.cursor.sh/oauth/token 和内置 client_id 交换新 token。
const CURSOR_OAUTH_TOKEN_URL: &str = "https://api2.cursor.sh/oauth/token";
const CURSOR_AUTH_CLIENT_ID: &str = "KbZUR41cY7W6zRSdpSUJ7I7mLYBKOCmB";

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CursorUserMetaResponse {
    email: Option<String>,
    sign_up_type: Option<String>,
    workos_id: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CursorStripeProfileResponse {
    membership_type: Option<String>,
    individual_membership_type: Option<String>,
    subscription_status: Option<String>,
    team_membership_type: Option<String>,
    is_team_member: Option<bool>,
    is_enterprise: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
struct CursorRefreshTokenResponse {
    #[serde(alias = "accessToken")]
    access_token: Option<String>,
    #[serde(alias = "refreshToken")]
    refresh_token: Option<String>,
    #[serde(default, alias = "shouldLogout")]
    should_logout: bool,
}

fn build_cursor_http_client() -> Result<reqwest::Client, String> {
    Ok(CURSOR_HTTP_CLIENT.clone())
}

fn extract_workos_user_id(jwt: &str) -> Option<String> {
    let value = decode_access_token_payload(jwt)?;
    let sub = value.get("sub")?.as_str()?;
    let user_id = sub.rsplit('|').next().unwrap_or(sub);
    if user_id.starts_with("user_") {
        Some(user_id.to_string())
    } else {
        None
    }
}

fn build_session_cookie(access_token: &str) -> Option<String> {
    let user_id = extract_workos_user_id(access_token)?;
    Some(format!(
        "WorkosCursorSessionToken={}%3A%3A{}",
        user_id, access_token
    ))
}

pub fn is_workos_session_token(raw: &str) -> bool {
    normalize_workos_session_token(raw).is_some()
}

pub fn normalize_workos_session_token(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let decoded = urlencoding::decode(trimmed)
        .map(|value| value.into_owned())
        .unwrap_or_else(|_| trimmed.to_string());
    let (user_id, jwt) = decoded.split_once("::")?;
    if user_id.starts_with("user_") && jwt.starts_with("eyJ") {
        Some(format!("{}::{}", user_id, jwt))
    } else {
        None
    }
}

pub fn read_workos_session_token(account: &CursorAccount) -> Option<String> {
    if let Some(Value::Object(raw)) = account.cursor_auth_raw.as_ref() {
        for key in ["workosSessionToken", "workos_cursor_session_token", "workosToken"] {
            if let Some(Value::String(token)) = raw.get(key) {
                if let Some(normalized) = normalize_workos_session_token(token) {
                    return Some(normalized);
                }
            }
        }
    }
    build_session_cookie(&account.access_token).and_then(|_| {
        let user_id = extract_workos_user_id(&account.access_token)?;
        Some(format!("{}::{}", user_id, account.access_token))
    })
}

fn generate_pkce_verifier_and_challenge() -> (String, String) {
    use rand::RngCore;
    use sha2::{Digest, Sha256};

    let mut verifier_bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut verifier_bytes);
    let verifier = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(verifier_bytes);
    let hash = Sha256::digest(verifier.as_bytes());
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(hash);
    (verifier, challenge)
}

async fn trigger_workos_authorization_login(
    client: &reqwest::Client,
    uuid: &str,
    challenge: &str,
    workos_session_token: &str,
) -> Result<(), String> {
    let cookie_value = format!("WorkosCursorSessionToken={}", workos_session_token);
    let response = client
        .post("https://cursor.com/api/auth/loginDeepCallbackControl")
        .header("Cookie", cookie_value)
        .header("Content-Type", "application/json")
        .header("Origin", "https://cursor.com")
        .json(&serde_json::json!({
            "challenge": challenge,
            "uuid": uuid,
        }))
        .send()
        .await
        .map_err(|e| format!("WorkOS 授权登录请求失败: {}", e))?;

    if !response.status().is_success() {
        let status = response.status().as_u16();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("WorkOS 授权登录失败: HTTP {} {}", status, body));
    }
    Ok(())
}

async fn poll_workos_authorization_tokens(
    client: &reqwest::Client,
    uuid: &str,
    verifier: &str,
) -> Result<(String, Option<String>), String> {
    for _ in 0..20 {
        let response = client
            .get(format!(
                "https://api2.cursor.sh/auth/poll?uuid={}&verifier={}",
                uuid, verifier
            ))
            .header("Accept", "*/*")
            .header("Content-Type", "application/json")
            .header("Origin", "https://cursor.com")
            .send()
            .await
            .map_err(|e| format!("WorkOS 授权轮询失败: {}", e))?;

        if response.status().is_success() {
            let body = response
                .text()
                .await
                .map_err(|e| format!("读取 WorkOS 授权响应失败: {}", e))?;
            if body.trim().is_empty() {
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                continue;
            }
            let value: Value = serde_json::from_str(&body)
                .map_err(|e| format!("解析 WorkOS 授权响应失败: {}", e))?;
            let access_token = value
                .get("accessToken")
                .or_else(|| value.get("access_token"))
                .and_then(|v| v.as_str())
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty());
            if let Some(access_token) = access_token {
                let refresh_token = value
                    .get("refreshToken")
                    .or_else(|| value.get("refresh_token"))
                    .and_then(|v| v.as_str())
                    .map(|v| v.trim().to_string())
                    .filter(|v| !v.is_empty());
                return Ok((access_token, refresh_token));
            }
        }

        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }

    Err("WorkOS 授权超时，未能换取 Access Token".to_string())
}

pub async fn exchange_workos_session_token(
    workos_session_token: &str,
) -> Result<(String, Option<String>), String> {
    let normalized = normalize_workos_session_token(workos_session_token)
        .ok_or_else(|| "无效的 WorkOS Session Token 格式".to_string())?;
    let client = build_cursor_http_client()?;
    let uuid = uuid::Uuid::new_v4().to_string();
    let (verifier, challenge) = generate_pkce_verifier_and_challenge();

    let poll_client = client.clone();
    let poll_uuid = uuid.clone();
    let poll_verifier = verifier.clone();
    let poll_task = tokio::spawn(async move {
        poll_workos_authorization_tokens(&poll_client, &poll_uuid, &poll_verifier).await
    });

    trigger_workos_authorization_login(&client, &uuid, &challenge, &normalized).await?;

    match poll_task.await {
        Ok(result) => result,
        Err(err) => Err(format!("WorkOS 授权任务失败: {}", err)),
    }
}

fn build_payload_from_workos_session_token(
    workos_session_token: &str,
    access_token: String,
    refresh_token: Option<String>,
) -> CursorImportPayload {
    let normalized = normalize_workos_session_token(workos_session_token).unwrap_or_else(|| {
        workos_session_token.trim().to_string()
    });
    let auth_id = normalized
        .split_once("::")
        .map(|(user_id, _)| user_id.to_string())
        .or_else(|| extract_auth_id_from_access_token(&access_token));
    let mut auth_raw = serde_json::Map::new();
    auth_raw.insert(
        "workosSessionToken".to_string(),
        Value::String(normalized),
    );

    CursorImportPayload {
        email: "unknown".to_string(),
        auth_id,
        name: None,
        access_token,
        refresh_token,
        membership_type: None,
        subscription_status: None,
        sign_up_type: None,
        cursor_auth_raw: Some(Value::Object(auth_raw)),
        cursor_usage_raw: None,
        cursor_credit_grants_raw: None,
        cursor_free_credit_usage_raw: None,
        status: None,
        status_reason: None,
        tags: None,
    }
}

pub async fn add_account_with_workos_token(
    workos_session_token: &str,
) -> Result<CursorAccount, String> {
    let normalized = normalize_workos_session_token(workos_session_token)
        .ok_or_else(|| "无效的 WorkOS Session Token 格式，应为 user_xxx::eyJ...".to_string())?;

    let (access_token, refresh_token) = match exchange_workos_session_token(&normalized).await {
        Ok(tokens) => tokens,
        Err(err) => {
            logger::log_warn(&format!(
                "[Cursor WorkOS] 授权换取失败，回退为直接解析 JWT: {}",
                err
            ));
            let jwt = normalized
                .split_once("::")
                .map(|(_, jwt)| jwt.to_string())
                .ok_or_else(|| err)?;
            (jwt, None)
        }
    };

    let payload = build_payload_from_workos_session_token(&normalized, access_token, refresh_token);
    upsert_account(payload)
}

fn resolve_membership_from_stripe_profile(profile: &CursorStripeProfileResponse) -> Option<String> {
    let membership = normalize_non_empty(profile.membership_type.as_deref());
    let individual = normalize_non_empty(profile.individual_membership_type.as_deref());

    if let Some(individual_value) = individual.as_ref() {
        if !individual_value.eq_ignore_ascii_case("free")
            && !matches!(
                membership.as_deref(),
                Some(value) if value.eq_ignore_ascii_case("enterprise")
            )
        {
            return Some(individual_value.clone());
        }
    }

    membership.or(individual)
}

async fn exchange_refresh_token_with_client(
    client: &reqwest::Client,
    refresh_token: &str,
) -> Result<CursorRefreshTokenResponse, String> {
    let response = client
        .post(CURSOR_OAUTH_TOKEN_URL)
        .header("Content-Type", "application/json")
        .json(&serde_json::json!({
            "grant_type": "refresh_token",
            "client_id": CURSOR_AUTH_CLIENT_ID,
            "refresh_token": refresh_token,
        }))
        .send()
        .await
        .map_err(|e| format!("请求 Cursor token 刷新接口失败: {}", e))?;

    let status = response.status().as_u16();
    let body = response
        .text()
        .await
        .map_err(|e| format!("读取 Cursor token 刷新响应失败: {}", e))?;

    if status == 401 || status == 403 {
        return Err("Cursor refresh token 已过期或无效，请重新导入账号".to_string());
    }
    if status != 200 {
        let detail = body.trim();
        return Err(if detail.is_empty() {
            format!("Cursor token 刷新接口返回异常状态码: {}", status)
        } else {
            format!(
                "Cursor token 刷新接口返回异常状态码: {}, body_len={}",
                status,
                body.len()
            )
        });
    }

    serde_json::from_str::<CursorRefreshTokenResponse>(&body)
        .map_err(|e| format!("解析 Cursor token 刷新响应失败: {}", e))
}

async fn refresh_account_access_token_with_client(
    client: &reqwest::Client,
    account: &mut CursorAccount,
) -> Result<bool, String> {
    let Some(refresh_token) = normalize_non_empty(account.refresh_token.as_deref()) else {
        return Ok(false);
    };

    let response = exchange_refresh_token_with_client(client, refresh_token.as_str()).await?;
    if response.should_logout {
        return Err("Cursor refresh token 已失效，请重新导入账号".to_string());
    }

    let new_access_token = normalize_non_empty(response.access_token.as_deref())
        .ok_or_else(|| "Cursor token 刷新响应缺少 access_token".to_string())?;
    let new_refresh_token =
        normalize_non_empty(response.refresh_token.as_deref()).or(Some(refresh_token));

    account.access_token = new_access_token.clone();
    account.refresh_token = new_refresh_token.clone();
    upsert_cursor_auth_raw_string(account, "accessToken", Some(new_access_token));
    upsert_cursor_auth_raw_string(account, "refreshToken", new_refresh_token);
    Ok(true)
}

async fn fetch_user_meta_with_client(
    client: &reqwest::Client,
    access_token: &str,
) -> Result<CursorUserMetaResponse, String> {
    let response = client
        .post(CURSOR_GET_USER_META_URL)
        .header("Authorization", format!("Bearer {}", access_token))
        .header("Accept", "application/json")
        .header("Content-Type", "application/json")
        .json(&serde_json::json!({}))
        .send()
        .await
        .map_err(|e| format!("请求 Cursor user meta 失败: {}", e))?;

    let status = response.status().as_u16();
    if status == 401 || status == 403 {
        return Err("Cursor 会话已过期或未认证，请重新导入账号".to_string());
    }
    if status != 200 {
        return Err(format!("Cursor user meta API 返回异常状态码: {}", status));
    }

    let body = response
        .text()
        .await
        .map_err(|e| format!("读取 Cursor user meta 响应失败: {}", e))?;

    serde_json::from_str::<CursorUserMetaResponse>(&body)
        .map_err(|e| format!("解析 Cursor user meta JSON 失败: {}", e))
}

async fn fetch_stripe_profile_with_client(
    client: &reqwest::Client,
    access_token: &str,
) -> Result<Option<CursorStripeProfileResponse>, String> {
    let full_response = client
        .get(CURSOR_FULL_STRIPE_PROFILE_URL)
        .header("Authorization", format!("Bearer {}", access_token))
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| format!("请求 Cursor full stripe profile 失败: {}", e))?;

    let full_status = full_response.status().as_u16();
    if full_status == 401 || full_status == 403 {
        return Err("Cursor 会话已过期或未认证，请重新导入账号".to_string());
    }
    if full_status == 200 {
        let body = full_response
            .text()
            .await
            .map_err(|e| format!("读取 Cursor full stripe profile 响应失败: {}", e))?;
        let profile = serde_json::from_str::<CursorStripeProfileResponse>(&body)
            .map_err(|e| format!("解析 Cursor full stripe profile JSON 失败: {}", e))?;
        return Ok(Some(profile));
    }

    let fallback_response = client
        .get(CURSOR_STRIPE_PROFILE_URL)
        .header("Authorization", format!("Bearer {}", access_token))
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| format!("请求 Cursor stripe profile 失败: {}", e))?;

    let fallback_status = fallback_response.status().as_u16();
    if fallback_status == 401 || fallback_status == 403 {
        return Err("Cursor 会话已过期或未认证，请重新导入账号".to_string());
    }
    if fallback_status != 200 {
        return Ok(None);
    }

    let body = fallback_response
        .text()
        .await
        .map_err(|e| format!("读取 Cursor stripe profile 响应失败: {}", e))?;

    let parsed = serde_json::from_str::<serde_json::Value>(&body)
        .map_err(|e| format!("解析 Cursor stripe profile JSON 失败: {}", e))?;

    match parsed {
        Value::Object(_) => serde_json::from_value::<CursorStripeProfileResponse>(parsed)
            .map(Some)
            .map_err(|e| format!("解析 Cursor stripe profile 对象失败: {}", e)),
        Value::String(text) => {
            if text.trim().is_empty() {
                Ok(None)
            } else {
                Ok(Some(CursorStripeProfileResponse {
                    membership_type: Some("pro".to_string()),
                    individual_membership_type: None,
                    subscription_status: None,
                    team_membership_type: None,
                    is_team_member: None,
                    is_enterprise: None,
                }))
            }
        }
        _ => Ok(None),
    }
}

async fn fetch_usage_summary_with_client(
    client: &reqwest::Client,
    access_token: &str,
) -> Result<serde_json::Value, String> {
    let cookie = build_session_cookie(access_token)
        .ok_or_else(|| "无法从 accessToken 解析 WorkOS 用户 ID".to_string())?;

    let response = client
        .get(CURSOR_USAGE_SUMMARY_URL)
        .header("Accept", "application/json")
        .header("Cookie", &cookie)
        .header(
            "User-Agent",
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)",
        )
        .send()
        .await
        .map_err(|e| format!("请求 Cursor usage API 失败: {}", e))?;

    let status = response.status().as_u16();
    if status == 401 || status == 403 {
        return Err("Cursor 会话已过期或未认证，请重新导入账号".to_string());
    }
    if status != 200 {
        return Err(format!("Cursor usage API 返回异常状态码: {}", status));
    }

    let body = response
        .text()
        .await
        .map_err(|e| format!("读取 Cursor usage 响应失败: {}", e))?;

    serde_json::from_str::<serde_json::Value>(&body)
        .map_err(|e| format!("解析 Cursor usage JSON 失败: {}", e))
}

async fn post_dashboard_json_with_client(
    client: &reqwest::Client,
    access_token: &str,
    url: &str,
    referer: &str,
) -> Result<serde_json::Value, String> {
    let cookie = build_session_cookie(access_token)
        .ok_or_else(|| "无法从 accessToken 解析 WorkOS 用户 ID".to_string())?;

    let response = client
        .post(url)
        .header("Accept", "application/json")
        .header("Content-Type", "application/json")
        .header("Cookie", &cookie)
        .header("Origin", "https://cursor.com")
        .header("Referer", referer)
        .header(
            "User-Agent",
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/148.0.0.0 Safari/537.36",
        )
        .json(&serde_json::json!({}))
        .send()
        .await
        .map_err(|e| format!("请求 Cursor dashboard API 失败: {}", e))?;

    let status = response.status().as_u16();
    if status == 401 || status == 403 {
        return Err(format!(
            "Cursor 会话已过期或未认证，请重新导入账号 (HTTP {})",
            status
        ));
    }
    if status != 200 {
        return Err(format!("Cursor dashboard API 返回异常状态码: {}", status));
    }

    let body = response
        .text()
        .await
        .map_err(|e| format!("读取 Cursor dashboard 响应失败: {}", e))?;

    serde_json::from_str::<serde_json::Value>(&body)
        .map_err(|e| format!("解析 Cursor dashboard JSON 失败: {}", e))
}

const CURSOR_USAGE_EVENT_KIND_FREE_CREDIT: &str = "USAGE_EVENT_KIND_FREE_CREDIT";
const CURSOR_FILTERED_USAGE_EVENTS_URL: &str =
    "https://cursor.com/api/dashboard/get-filtered-usage-events";
const CURSOR_STRIPE_SESSION_URL: &str = "https://cursor.com/api/stripeSession";
const CURSOR_AUTH_SESSIONS_URL: &str = "https://cursor.com/api/auth/sessions";
const CURSOR_AUTH_SESSIONS_REVOKE_URL: &str = "https://cursor.com/api/auth/sessions/revoke";
const CURSOR_AUTH_SESSIONS_REFERER: &str = "https://cursor.com/dashboard/settings";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CursorAuthSession {
    pub session_id: String,
    #[serde(rename = "type")]
    pub session_type: String,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
}

fn parse_usage_timestamp_ms(value: &serde_json::Value) -> Option<i64> {
    if let Some(n) = value.as_i64() {
        return Some(if n > 1_000_000_000_000 {
            n
        } else {
            n * 1000
        });
    }
    if let Some(n) = value.as_u64() {
        let n = n as i64;
        return Some(if n > 1_000_000_000_000 {
            n
        } else {
            n * 1000
        });
    }
    if let Some(text) = value.as_str() {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return None;
        }
        if let Ok(n) = trimmed.parse::<i64>() {
            return Some(if n > 1_000_000_000_000 {
                n
            } else {
                n * 1000
            });
        }
        if let Ok(n) = trimmed.parse::<f64>() {
            let n = n as i64;
            return Some(if n > 1_000_000_000_000 {
                n
            } else {
                n * 1000
            });
        }
        if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(trimmed) {
            return Some(dt.timestamp_millis());
        }
    }
    None
}

fn extract_latest_usage_event_ts_ms(root: &serde_json::Value) -> Option<i64> {
    let events = root
        .get("usageEventsDisplay")
        .or_else(|| root.get("usage_events_display"))
        .and_then(|value| value.as_array())?;
    events
        .iter()
        .filter_map(|event| event.get("timestamp").and_then(parse_usage_timestamp_ms))
        .max()
}

async fn fetch_latest_usage_event_ts_with_client(
    client: &reqwest::Client,
    account: &CursorAccount,
) -> Result<Option<i64>, String> {
    let empty_usage = serde_json::json!({});
    let usage_ref = account.cursor_usage_raw.as_ref().unwrap_or(&empty_usage);
    let (start_date, end_date) = resolve_billing_cycle_range_ms(usage_ref);
    let body = serde_json::json!({
        "teamId": 0,
        "startDate": start_date,
        "endDate": end_date,
        "page": 1,
        "pageSize": 20
    });
    let response = post_dashboard_json_body_with_client(
        client,
        account,
        CURSOR_FILTERED_USAGE_EVENTS_URL,
        "https://cursor.com/dashboard",
        body,
    )
    .await?;
    Ok(extract_latest_usage_event_ts_ms(&response))
}

fn resolve_billing_cycle_range_ms(usage_raw: &serde_json::Value) -> (String, String) {
    let now_ms = chrono::Utc::now().timestamp_millis();
    let cycle_end_ms = usage_raw
        .get("billingCycleEnd")
        .or_else(|| usage_raw.get("billing_cycle_end"))
        .and_then(parse_usage_timestamp_ms)
        .unwrap_or(now_ms);
    let end_ms = now_ms.min(cycle_end_ms);

    let default_start_ms = end_ms - 30_i64 * 24 * 60 * 60 * 1000;
    let start_ms = usage_raw
        .get("billingCycleStart")
        .or_else(|| usage_raw.get("billing_cycle_start"))
        .and_then(parse_usage_timestamp_ms)
        .unwrap_or(default_start_ms)
        .min(end_ms);

    (start_ms.to_string(), end_ms.to_string())
}

fn parse_dollar_string_to_cents(text: &str) -> i64 {
    let cleaned = text.trim().replace(',', "").replace('$', "");
    if cleaned.is_empty() {
        return 0;
    }
    if let Ok(value) = cleaned.parse::<f64>() {
        return (value * 100.0).round() as i64;
    }
    0
}

/// Auto-routing (`default`), Composer, and Grok models are not gifted-credit API usage.
fn is_gift_credit_usage_model(model: &str) -> bool {
    let normalized = model.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return false;
    }
    if normalized == "default" {
        return false;
    }
    if normalized.starts_with("composer") {
        return false;
    }
    if normalized.contains("grok") {
        return false;
    }
    true
}

fn free_credit_event_cents(event: &serde_json::Value) -> i64 {
    // FREE_CREDIT fair-value: prefer tokenUsage.totalCents (Cursor often bills "$0.00"
    // while still reporting promotional usage value in totalCents).
    if let Some(token) = event
        .get("tokenUsage")
        .or_else(|| event.get("token_usage"))
    {
        if let Some(cents) = json_get_cents(token, &["totalCents", "total_cents"]) {
            if cents > 0 {
                return cents;
            }
        }
    }
    if let Some(text) = event
        .get("usageBasedCosts")
        .or_else(|| event.get("usage_based_costs"))
        .and_then(|value| value.as_str())
    {
        let parsed = parse_dollar_string_to_cents(text);
        if parsed > 0 {
            return parsed;
        }
    }
    0
}

fn sum_free_credit_cents_from_events_page(root: &serde_json::Value) -> (i64, i32) {
    let events = root
        .get("usageEventsDisplay")
        .or_else(|| root.get("usage_events_display"))
        .and_then(|value| value.as_array());

    let mut sum = 0_i64;
    let mut count = 0_i32;
    if let Some(events) = events {
        for event in events {
            let kind = event
                .get("kind")
                .and_then(|value| value.as_str())
                .unwrap_or("");
            if kind != CURSOR_USAGE_EVENT_KIND_FREE_CREDIT {
                continue;
            }
            let model = event
                .get("model")
                .and_then(|value| value.as_str())
                .unwrap_or("");
            if !is_gift_credit_usage_model(model) {
                continue;
            }
            let cents = free_credit_event_cents(event);
            if cents <= 0 {
                continue;
            }
            count += 1;
            sum += cents;
        }
    }
    (sum, count)
}

async fn post_dashboard_json_body_with_client(
    client: &reqwest::Client,
    account: &CursorAccount,
    url: &str,
    referer: &str,
    body: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let cookie = account_dashboard_cookie(account)?;
    let response = client
        .post(url)
        .header("Accept", "application/json")
        .header("Content-Type", "application/json")
        .header("Cookie", &cookie)
        .header("Origin", "https://cursor.com")
        .header("Referer", referer)
        .header(
            "User-Agent",
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/148.0.0.0 Safari/537.36",
        )
        .json(&body)
        .timeout(std::time::Duration::from_secs(40))
        .send()
        .await
        .map_err(|e| format!("请求 Cursor dashboard API 失败: {}", e))?;

    let status = response.status().as_u16();
    if status == 401 || status == 403 {
        return Err(format!(
            "Cursor 会话已过期或未认证，请重新导入账号 (HTTP {})",
            status
        ));
    }
    if status != 200 {
        return Err(format!("Cursor dashboard API 返回异常状态码: {}", status));
    }

    let body = response
        .text()
        .await
        .map_err(|e| format!("读取 Cursor dashboard 响应失败: {}", e))?;

    serde_json::from_str::<serde_json::Value>(&body)
        .map_err(|e| format!("解析 Cursor dashboard JSON 失败: {}", e))
}

async fn fetch_free_credit_usage_with_client(
    client: &reqwest::Client,
    account: &CursorAccount,
    usage_raw: Option<&serde_json::Value>,
) -> Result<Option<serde_json::Value>, String> {
    let empty_usage = serde_json::json!({});
    let usage_ref = usage_raw.unwrap_or(&empty_usage);
    let (start_date, end_date) = resolve_billing_cycle_range_ms(usage_ref);
    let page_size = FREE_CREDIT_USAGE_EVENTS_PAGE_SIZE;
    let mut page = 1_i32;
    let mut used_cents = 0_i64;
    let mut free_credit_event_count = 0_i32;
    let mut fetched_events = 0_i32;
    let mut total_events = 0_i32;

    loop {
        let body = serde_json::json!({
            "teamId": 0,
            "startDate": start_date,
            "endDate": end_date,
            "page": page,
            "pageSize": page_size
        });
        let response = post_dashboard_json_body_with_client(
            client,
            account,
            CURSOR_FILTERED_USAGE_EVENTS_URL,
            "https://cursor.com/dashboard",
            body,
        )
        .await?;

        let page_total = json_get_i64(
            &response,
            &["totalUsageEventsCount", "total_usage_events_count"],
        )
        .unwrap_or(0) as i32;
        if page == 1 {
            total_events = page_total;
        }

        let events = response
            .get("usageEventsDisplay")
            .or_else(|| response.get("usage_events_display"))
            .and_then(|value| value.as_array());
        let page_event_count = events.map(|items| items.len() as i32).unwrap_or(0);
        fetched_events += page_event_count;

        let (page_sum, page_count) = sum_free_credit_cents_from_events_page(&response);
        used_cents += page_sum;
        free_credit_event_count += page_count;

        if page_event_count == 0 || fetched_events >= total_events || total_events == 0 {
            break;
        }
        page += 1;
    }

    if free_credit_event_count == 0 {
        return Ok(None);
    }

    Ok(Some(serde_json::json!({
        "usedCents": used_cents,
        "startDate": start_date,
        "endDate": end_date,
        "eventCount": free_credit_event_count,
        "source": "free_credit_events",
    })))
}

fn account_dashboard_cookie(account: &CursorAccount) -> Result<String, String> {
    if let Some(token) = read_workos_session_token(account) {
        return Ok(format!("WorkosCursorSessionToken={}", token));
    }
    build_session_cookie(&account.access_token)
        .ok_or_else(|| "无法获取 WorkOS Session Token，请重新导入账号".to_string())
}

async fn post_dashboard_json_body_for_account(
    account: &CursorAccount,
    url: &str,
    referer: &str,
    body: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let client = build_cursor_http_client()?;
    post_dashboard_json_body_with_client(&client, account, url, referer, body).await
}

pub async fn fetch_cursor_aggregated_usage(
    account_id: &str,
    start_date: u64,
    end_date: u64,
    team_id: i32,
) -> Result<serde_json::Value, String> {
    let account = load_account(account_id)
        .ok_or_else(|| format!("Cursor account not found: {}", account_id))?;
    let body = serde_json::json!({
        "startDate": start_date,
        "endDate": end_date,
        "teamId": team_id
    });
    post_dashboard_json_body_for_account(
        &account,
        "https://cursor.com/api/dashboard/get-aggregated-usage-events",
        "https://cursor.com/cn/dashboard",
        body,
    )
    .await
}

pub async fn fetch_cursor_usage_events(
    account_id: &str,
    team_id: i32,
    start_date: String,
    end_date: String,
    page: i32,
    page_size: i32,
) -> Result<serde_json::Value, String> {
    let account = load_account(account_id)
        .ok_or_else(|| format!("Cursor account not found: {}", account_id))?;
    let body = serde_json::json!({
        "teamId": team_id,
        "startDate": start_date,
        "endDate": end_date,
        "page": page,
        "pageSize": page_size
    });
    post_dashboard_json_body_for_account(
        &account,
        "https://cursor.com/api/dashboard/get-filtered-usage-events",
        "https://cursor.com/dashboard",
        body,
    )
    .await
}

pub async fn fetch_cursor_user_analytics(
    account_id: &str,
    team_id: i32,
    user_id: i32,
    start_date: String,
    end_date: String,
) -> Result<serde_json::Value, String> {
    let account = load_account(account_id)
        .ok_or_else(|| format!("Cursor account not found: {}", account_id))?;
    let body = serde_json::json!({
        "teamId": team_id,
        "userId": user_id,
        "startDate": start_date,
        "endDate": end_date
    });
    post_dashboard_json_body_for_account(
        &account,
        "https://cursor.com/api/dashboard/get-user-analytics",
        "https://cursor.com/dashboard",
        body,
    )
    .await
}

fn parse_cursor_auth_sessions(raw: &serde_json::Value) -> Vec<CursorAuthSession> {
    let Some(list) = raw
        .get("sessions")
        .and_then(|value| value.as_array())
    else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|item| {
            let session_id = item
                .get("sessionId")
                .or_else(|| item.get("session_id"))
                .and_then(|value| value.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())?
                .to_string();
            let session_type = parse_cursor_auth_session_type(item);
            let created_at = item
                .get("createdAt")
                .or_else(|| item.get("created_at"))
                .and_then(|value| value.as_str())
                .unwrap_or("")
                .to_string();
            let expires_at = item
                .get("expiresAt")
                .or_else(|| item.get("expires_at"))
                .and_then(|value| value.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string);
            Some(CursorAuthSession {
                session_id,
                session_type,
                created_at,
                expires_at,
            })
        })
        .collect()
}

fn is_safe_session_id(session_id: &str) -> bool {
    let trimmed = session_id.trim();
    !trimmed.is_empty()
        && trimmed.len() <= 128
        && trimmed
            .chars()
            .all(|ch| ch.is_ascii_hexdigit() || ch == '-' || ch == '_')
}

fn parse_cursor_auth_session_type(item: &serde_json::Value) -> String {
    let value = item
        .get("type")
        .or_else(|| item.get("sessionType"))
        .or_else(|| item.get("session_type"));
    if let Some(text) = value.and_then(|raw| raw.as_str()) {
        return text.trim().to_string();
    }
    if let Some(n) = value.and_then(|raw| raw.as_i64()) {
        return match n {
            1 => "SESSION_TYPE_WEB".to_string(),
            2 => "SESSION_TYPE_CLIENT".to_string(),
            _ => n.to_string(),
        };
    }
    String::new()
}

fn cursor_auth_session_revoke_type(session_type: &str) -> Result<String, String> {
    let trimmed = session_type.trim();
    if trimmed.is_empty() {
        return Err("无法识别会话类型，无法撤销".to_string());
    }
    if trimmed == "SESSION_TYPE_WEB" || trimmed == "SESSION_TYPE_CLIENT" {
        return Ok(trimmed.to_string());
    }
    let upper = trimmed.to_ascii_uppercase();
    if upper == "1" || upper.contains("WEB") {
        return Ok("SESSION_TYPE_WEB".to_string());
    }
    if upper == "2" || upper.contains("CLIENT") || upper.contains("DESKTOP") {
        return Ok("SESSION_TYPE_CLIENT".to_string());
    }
    Err("无法识别会话类型，无法撤销".to_string())
}

fn cursor_auth_session_revoke_payload(
    session_id: &str,
    session_type: &str,
) -> Result<serde_json::Value, String> {
    Ok(serde_json::json!({
        "sessionId": session_id.trim(),
        "type": cursor_auth_session_revoke_type(session_type)?,
    }))
}

fn cursor_auth_session_revoke_succeeded(raw: &serde_json::Value) -> bool {
    raw.get("success").and_then(|value| value.as_bool()) == Some(true)
}

async fn get_json_for_account(
    account: &CursorAccount,
    url: &str,
    referer: &str,
) -> Result<serde_json::Value, String> {
    let client = build_cursor_http_client()?;
    let cookie = account_dashboard_cookie(account)?;
    let response = client
        .get(url)
        .header("Accept", "application/json")
        .header("Cookie", &cookie)
        .header("Origin", "https://cursor.com")
        .header("Referer", referer)
        .header(
            "User-Agent",
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/148.0.0.0 Safari/537.36",
        )
        .timeout(std::time::Duration::from_secs(40))
        .send()
        .await
        .map_err(|e| format!("请求 Cursor 会话接口失败: {}", e))?;

    let status = response.status().as_u16();
    if status == 401 || status == 403 {
        return Err(format!(
            "Cursor 会话已过期或未认证，请重新导入账号 (HTTP {})",
            status
        ));
    }
    if status != 200 {
        return Err(format!("Cursor 会话接口返回异常状态码: {}", status));
    }
    let body = response
        .text()
        .await
        .map_err(|e| format!("读取 Cursor 会话响应失败: {}", e))?;
    serde_json::from_str::<serde_json::Value>(&body)
        .map_err(|e| format!("解析 Cursor 会话 JSON 失败: {}", e))
}

pub async fn fetch_cursor_auth_sessions(account_id: &str) -> Result<Vec<CursorAuthSession>, String> {
    let account = load_account(account_id)
        .ok_or_else(|| format!("Cursor account not found: {}", account_id))?;
    let raw = get_json_for_account(&account, CURSOR_AUTH_SESSIONS_URL, CURSOR_AUTH_SESSIONS_REFERER)
        .await?;
    Ok(parse_cursor_auth_sessions(&raw))
}

pub async fn revoke_cursor_auth_session(
    account_id: &str,
    session_id: &str,
    session_type: Option<&str>,
) -> Result<Vec<CursorAuthSession>, String> {
    if !is_safe_session_id(session_id) {
        return Err("会话 ID 无效".to_string());
    }
    let session_id = session_id.trim();
    let resolved_type = match session_type.map(str::trim).filter(|value| !value.is_empty()) {
        Some(value) => value.to_string(),
        None => {
            let sessions = fetch_cursor_auth_sessions(account_id).await?;
            sessions
                .iter()
                .find(|item| item.session_id == session_id)
                .map(|item| item.session_type.clone())
                .ok_or_else(|| "会话不存在或已失效".to_string())?
        }
    };
    let account = load_account(account_id)
        .ok_or_else(|| format!("Cursor account not found: {}", account_id))?;
    let payload = cursor_auth_session_revoke_payload(session_id, &resolved_type)?;
    let raw = post_dashboard_json_body_for_account(
        &account,
        CURSOR_AUTH_SESSIONS_REVOKE_URL,
        CURSOR_AUTH_SESSIONS_REFERER,
        payload,
    )
    .await?;
    if !cursor_auth_session_revoke_succeeded(&raw) {
        return Err("撤销未生效，接口未确认成功".to_string());
    }
    fetch_cursor_auth_sessions(account_id).await
}

fn json_get_i64(value: &serde_json::Value, keys: &[&str]) -> Option<i64> {
    json_get_cents(value, keys)
}

fn json_get_cents(value: &serde_json::Value, keys: &[&str]) -> Option<i64> {
    let obj = value.as_object()?;
    for key in keys {
        let Some(raw) = obj.get(*key) else {
            continue;
        };
        if let Some(n) = raw.as_i64() {
            return Some(n);
        }
        if let Some(n) = raw.as_u64() {
            return Some(n as i64);
        }
        if let Some(n) = raw.as_f64() {
            if n.is_finite() {
                return Some(n.round() as i64);
            }
        }
        if let Some(text) = raw.as_str() {
            let trimmed = text.trim();
            if let Ok(n) = trimmed.parse::<i64>() {
                return Some(n);
            }
            if let Ok(n) = trimmed.parse::<f64>() {
                if n.is_finite() {
                    return Some(n.round() as i64);
                }
            }
        }
    }
    None
}

fn json_get_bool(value: &serde_json::Value, keys: &[&str]) -> Option<bool> {
    let obj = value.as_object()?;
    for key in keys {
        if let Some(raw) = obj.get(*key) {
            if let Some(flag) = raw.as_bool() {
                return Some(flag);
            }
        }
    }
    None
}

fn credit_grants_balance_node(root: &serde_json::Value) -> &serde_json::Value {
    root.get("balance")
        .filter(|value| value.is_object())
        .unwrap_or(root)
}

fn extract_credit_grants_metrics(root: &serde_json::Value) -> (Option<i64>, Option<i64>, Option<i64>) {
    let balance = credit_grants_balance_node(root);
    let total = json_get_i64(
        balance,
        &[
            "totalCents",
            "total_cents",
            "grantTotalCents",
            "grant_total_cents",
        ],
    );
    let remaining = json_get_i64(
        balance,
        &[
            "remainingCents",
            "remaining_cents",
            "balanceCents",
            "balance_cents",
            "creditBalanceCents",
            "credit_balance_cents",
        ],
    );
    let used = json_get_i64(
        balance,
        &["usedCents", "used_cents", "grantUsedCents", "grant_used_cents"],
    )
    .or_else(|| {
        total
            .zip(remaining)
            .map(|(total_cents, remaining_cents)| (total_cents - remaining_cents).max(0))
    });
    let remaining = remaining.or_else(|| {
        total
            .zip(used)
            .map(|(total_cents, used_cents)| (total_cents - used_cents).max(0))
    });
    (total, used, remaining)
}

fn credit_grants_has_active_display(root: &serde_json::Value) -> bool {
    credit_grants_has_valid_display_metrics(root)
}

/// Live credit-grants API returned usable balance fields (non-zero total/used/remaining).
/// When false, FREE_CREDIT usage events are the fallback display source — that event kind
/// only appears for accounts that have (or had) gifted credits.
fn credit_grants_has_valid_display_metrics(root: &serde_json::Value) -> bool {
    let (total, used, remaining) = extract_credit_grants_metrics(root);
    if remaining.unwrap_or(0) > 0 {
        return true;
    }
    if total.unwrap_or(0) > 0 {
        return true;
    }
    if used.unwrap_or(0) > 0 {
        return true;
    }
    false
}

/// Only skip FREE_CREDIT event aggregation when there is still an active remaining balance.
/// Exhausted/historical peaks must not block billing-cycle FREE_CREDIT usage (card + modal).
fn credit_grants_has_active_remaining(root: &serde_json::Value) -> bool {
    if root.get("historical").and_then(|value| value.as_bool()) == Some(true) {
        return false;
    }
    let balance = credit_grants_balance_node(root);
    if balance
        .get("historical")
        .and_then(|value| value.as_bool())
        == Some(true)
    {
        return false;
    }
    let (_total, _used, remaining) = extract_credit_grants_metrics(root);
    remaining.unwrap_or(0) > 0
}

fn build_credit_grants_peak(total: i64, used: Option<i64>, remaining: Option<i64>) -> serde_json::Value {
    let resolved_used = used
        .or_else(|| {
            remaining.map(|remaining_cents| (total - remaining_cents).max(0))
        })
        .unwrap_or(0)
        .max(0);
    serde_json::json!({
        "totalCents": total,
        "usedCents": resolved_used,
    })
}

fn resolve_credit_grants_peak(
    previous: Option<&serde_json::Value>,
    fresh: &serde_json::Value,
) -> Option<serde_json::Value> {
    if let Some(peak) = fresh.get("peak").filter(|value| value.is_object()) {
        let total = json_get_i64(peak, &["totalCents", "total_cents"]).unwrap_or(0);
        if total > 0 {
            return Some(peak.clone());
        }
    }
    if let Some(previous) = previous {
        if let Some(peak) = previous.get("peak").filter(|value| value.is_object()) {
            let total = json_get_i64(peak, &["totalCents", "total_cents"]).unwrap_or(0);
            if total > 0 {
                return Some(peak.clone());
            }
        }
    }

    for source in [Some(fresh), previous] {
        let Some(source) = source else { continue };
        let (total, used, remaining) = extract_credit_grants_metrics(source);
        if total.unwrap_or(0) > 0 {
            return Some(build_credit_grants_peak(
                total.unwrap_or(0),
                used,
                remaining,
            ));
        }
    }
    None
}

fn merge_credit_grants_preserving_history(
    previous: Option<&serde_json::Value>,
    fresh: serde_json::Value,
) -> serde_json::Value {
    let peak = resolve_credit_grants_peak(previous, &fresh);

    if credit_grants_has_active_display(&fresh) {
        let mut merged = fresh;
        if let Some(peak) = peak {
            if let serde_json::Value::Object(ref mut map) = merged {
                map.insert("peak".to_string(), peak);
            }
        }
        return merged;
    }

    let Some(peak) = peak else {
        return fresh;
    };
    let peak_total = json_get_i64(&peak, &["totalCents", "total_cents"]).unwrap_or(0);
    if peak_total <= 0 {
        return fresh;
    }
    let peak_used = json_get_i64(&peak, &["usedCents", "used_cents"]).unwrap_or(peak_total);

    serde_json::json!({
        "balance": {
            "totalCents": peak_total,
            "usedCents": peak_used.max(peak_total),
            "remainingCents": 0,
            "hasCreditGrants": true,
            "historical": true,
        },
        "grants": fresh
            .get("grants")
            .cloned()
            .or_else(|| previous.and_then(|value| value.get("grants").cloned()))
            .unwrap_or_else(|| serde_json::json!({})),
        "peak": peak,
        "historical": true,
    })
}

fn referral_has_code(value: &serde_json::Value) -> bool {
    let keys = [
        "referralCode",
        "referral_code",
        "code",
        "referralLink",
        "referral_link",
        "referralUrl",
        "referral_url",
    ];
    for key in keys {
        if let Some(text) = value.get(key).and_then(|raw| raw.as_str()) {
            if !text.trim().is_empty() {
                return true;
            }
        }
    }
    false
}

fn referral_is_eligible(value: &serde_json::Value) -> bool {
    if value.get("eligible").and_then(|raw| raw.as_bool()) == Some(false) {
        return false;
    }
    if referral_has_code(value) {
        return true;
    }
    json_get_bool(
        value,
        &[
            "eligible",
            "isEligible",
            "hasReferralProgram",
            "hasP2PReferral",
            "has_p2p_referral",
        ],
    ) == Some(true)
}

fn normalize_referral_status(mut response: serde_json::Value) -> serde_json::Value {
    let eligible = referral_is_eligible(&response);
    if let serde_json::Value::Object(ref mut map) = response {
        map.insert("eligible".to_string(), serde_json::json!(eligible));
    } else {
        response = serde_json::json!({ "eligible": eligible });
    }
    response
}

async fn fetch_referral_status_with_client(
    client: &reqwest::Client,
    access_token: &str,
) -> Result<serde_json::Value, String> {
    match post_dashboard_json_with_client(
        client,
        access_token,
        CURSOR_P2P_REFERRAL_STATUS_URL,
        "https://cursor.com/dashboard/referrals",
    )
    .await
    {
        Ok(response) => Ok(normalize_referral_status(response)),
        Err(err) if is_cursor_session_error(&err) => Err(err),
        Err(err) if err.contains("403") || err.contains("404") => {
            Ok(serde_json::json!({ "eligible": false }))
        }
        Err(err) => Err(err),
    }
}

pub async fn fetch_referral_status_async(account_id: &str) -> Result<CursorAccount, String> {
    let mut account = load_account(account_id).ok_or_else(|| "账号不存在".to_string())?;
    let client = build_cursor_http_client()?;

    if access_token_needs_refresh(&account.access_token) {
        let _ = refresh_account_access_token_with_client(&client, &mut account).await;
    }

    let referral = fetch_referral_status_with_client(&client, &account.access_token).await?;
    account.cursor_referral_raw = Some(referral);
    account.last_used = now_ts();
    let updated = upsert_refreshed_account_record(account)?;
    Ok(updated)
}

async fn fetch_credit_grants_with_client(
    client: &reqwest::Client,
    access_token: &str,
) -> Result<serde_json::Value, String> {
    let balance = post_dashboard_json_with_client(
        client,
        access_token,
        CURSOR_CREDIT_GRANTS_BALANCE_URL,
        "https://cursor.com/dashboard",
    )
    .await?;
    let grants = post_dashboard_json_with_client(
        client,
        access_token,
        CURSOR_CLIENT_VISIBLE_CREDIT_GRANTS_URL,
        "https://cursor.com/dashboard",
    )
    .await
    .unwrap_or_else(|err| {
        logger::log_warn(&format!(
            "[Cursor Refresh] 赠送额度明细拉取失败，仅保留余额: {}",
            err
        ));
        serde_json::json!({})
    });

    Ok(serde_json::json!({
        "balance": balance,
        "grants": grants,
    }))
}

async fn fetch_sand_usage_status_with_client(
    client: &reqwest::Client,
    access_token: &str,
) -> Result<serde_json::Value, String> {
    post_dashboard_json_with_client(
        client,
        access_token,
        CURSOR_SAND_USAGE_STATUS_URL,
        "https://cursor.com/dashboard/spending",
    )
    .await
}

async fn fetch_sand_access_status_with_client(
    client: &reqwest::Client,
    access_token: &str,
) -> Result<serde_json::Value, String> {
    post_dashboard_json_with_client(
        client,
        access_token,
        CURSOR_SAND_ACCESS_STATUS_URL,
        "https://cursor.com/dashboard/spending",
    )
    .await
}

fn is_cursor_session_error(err: &str) -> bool {
    err.contains("会话已过期") || err.contains("未认证") || err.contains("HTTP 401")
}

fn extract_sand_usage_percent(root: &serde_json::Value) -> Option<f64> {
    pick_number(Some(root), &["usagePercent", "usage_percent"]).filter(|value| value.is_finite())
}

fn sand_access_is_granted(access: &serde_json::Value) -> Option<bool> {
    let state = access
        .get("state")
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .trim();
    if state.is_empty() {
        return None;
    }
    Some(state.to_ascii_uppercase().contains("GRANTED"))
}

fn attach_sand_access(
    mut usage: serde_json::Value,
    access: Option<serde_json::Value>,
) -> serde_json::Value {
    if let Some(access) = access {
        if let serde_json::Value::Object(ref mut map) = usage {
            map.insert("access".to_string(), access);
        }
    }
    usage
}

fn parse_sand_reset_ts(root: &serde_json::Value) -> Option<i64> {
    root.get("nextResetTimestampUtc")
        .or_else(|| root.get("next_reset_timestamp_utc"))
        .or_else(|| root.get("sandTrialExpiresAt"))
        .and_then(|value| value.as_str())
        .and_then(|text| chrono::DateTime::parse_from_rfc3339(text).ok())
        .map(|value| value.timestamp())
}

pub(crate) fn cursor_grok_bot_display(account: &CursorAccount) -> Option<(i32, Option<i64>)> {
    let raw = account.cursor_sand_usage_raw.as_ref()?;
    if let Some(access) = raw.get("access") {
        if sand_access_is_granted(access) == Some(false) {
            return None;
        }
    }
    let percent = extract_sand_usage_percent(raw).map(clamp_percent)?;
    Some((percent, parse_sand_reset_ts(raw)))
}

fn apply_cursor_sand_refresh(
    account: &mut CursorAccount,
    sand_usage_result: Result<serde_json::Value, String>,
    sand_access: Option<serde_json::Value>,
    usage_refreshed: bool,
) {
    match sand_usage_result {
        Ok(sand_usage) => {
            let granted = sand_access.as_ref().and_then(sand_access_is_granted);
            if granted == Some(false) {
                if usage_refreshed {
                    account.cursor_sand_usage_raw = None;
                    logger::log_info(&format!(
                        "[Cursor Refresh] 账号无 Grok-Bot 权限，已隐藏: id={}",
                        account.id
                    ));
                } else {
                    logger::log_info(&format!(
                        "[Cursor Refresh] 配额刷新失败，保留上次 Grok-Bot 用量: id={}",
                        account.id
                    ));
                }
                return;
            }
            if extract_sand_usage_percent(&sand_usage).is_some() {
                account.cursor_sand_usage_raw =
                    Some(attach_sand_access(sand_usage, sand_access));
                logger::log_info(&format!(
                    "[Cursor Refresh] Grok-Bot 用量拉取成功: id={}",
                    account.id
                ));
            } else if usage_refreshed {
                account.cursor_sand_usage_raw = None;
                logger::log_info(&format!(
                    "[Cursor Refresh] Grok-Bot 用量无 usagePercent，已清空: id={}",
                    account.id
                ));
            } else {
                logger::log_info(&format!(
                    "[Cursor Refresh] 配额刷新失败且 Grok-Bot 无 usagePercent，保留上次用量: id={}",
                    account.id
                ));
            }
        }
        Err(err) if is_cursor_session_error(&err) => {
            logger::log_warn(&format!(
                "[Cursor Refresh] Grok-Bot 用量因会话失效未更新，保留上次状态: id={}, error={}",
                account.id, err
            ));
        }
        Err(err) if err.contains("404") || err.contains("403") => {
            if usage_refreshed {
                account.cursor_sand_usage_raw = None;
                logger::log_info(&format!(
                    "[Cursor Refresh] 账号无 Grok-Bot 用量: id={}, error={}",
                    account.id, err
                ));
            } else {
                logger::log_warn(&format!(
                    "[Cursor Refresh] Grok-Bot 用量拉取失败，保留上次状态: id={}, error={}",
                    account.id, err
                ));
            }
        }
        Err(err) => {
            logger::log_warn(&format!(
                "[Cursor Refresh] Grok-Bot 用量拉取失败: id={}, error={}",
                account.id, err
            ));
        }
    }
}

// ---------------------------------------------------------------------------
// Refresh (updates our own account storage + fetches usage from official APIs)
// ---------------------------------------------------------------------------

async fn refresh_account_async_once(account_id: &str) -> Result<CursorAccount, String> {
    let existing = load_account(account_id).ok_or_else(|| "账号不存在".to_string())?;
    logger::log_info(&format!(
        "[Cursor Refresh] 开始刷新账号: id={}, email={}",
        existing.id, existing.email
    ));

    let client = build_cursor_http_client()?;
    let mut account = existing.clone();

    if access_token_needs_refresh(&account.access_token) {
        match refresh_account_access_token_with_client(&client, &mut account).await {
            Ok(true) => {
                logger::log_info(&format!(
                    "[Cursor Refresh] access token 刷新成功: id={}",
                    account.id
                ));
            }
            Ok(false) => {}
            Err(err) => {
                logger::log_warn(&format!(
                    "[Cursor Refresh] access token 刷新失败，继续使用现有 token: id={}, error={}",
                    account.id, err
                ));
            }
        }
    }

    let access_token = account.access_token.clone();
    let request_account = account.clone();
    let (
        meta_result,
        stripe_result,
        usage_result,
        credit_grants_result,
        referral_result,
        sand_usage_result,
        sand_access_result,
        latest_usage_event_result,
    ) = tokio::join!(
        fetch_user_meta_with_client(&client, &access_token),
        fetch_stripe_profile_with_client(&client, &access_token),
        fetch_usage_summary_with_client(&client, &access_token),
        fetch_credit_grants_with_client(&client, &access_token),
        fetch_referral_status_with_client(&client, &access_token),
        fetch_sand_usage_status_with_client(&client, &access_token),
        fetch_sand_access_status_with_client(&client, &access_token),
        fetch_latest_usage_event_ts_with_client(&client, &request_account),
    );

    match meta_result {
        Ok(meta) => {
            if let Some(email) = normalize_email_identity(meta.email.as_deref()) {
                account.email = email.clone();
                upsert_cursor_auth_raw_string(&mut account, "cachedEmail", Some(email));
            }

            if let Some(sign_up_type) = normalize_cursor_sign_up_type(meta.sign_up_type.as_deref())
            {
                account.sign_up_type = Some(sign_up_type.clone());
                upsert_cursor_auth_raw_string(&mut account, "cachedSignUpType", Some(sign_up_type));
            }

            upsert_cursor_auth_raw_string(&mut account, "workosId", meta.workos_id.clone());
            if account.auth_id.is_none() {
                account.auth_id = normalize_non_empty(meta.workos_id.as_deref());
            }

            logger::log_info(&format!(
                "[Cursor Refresh] 用户信息拉取成功: id={}, email={}",
                account.id, account.email
            ));
        }
        Err(err) => {
            logger::log_warn(&format!(
                "[Cursor Refresh] 用户信息拉取失败: id={}, error={}",
                account.id, err
            ));
        }
    }

    match stripe_result {
        Ok(Some(profile)) => {
            if let Some(membership_type) = resolve_membership_from_stripe_profile(&profile) {
                account.membership_type = Some(membership_type.clone());
                upsert_cursor_auth_raw_string(
                    &mut account,
                    "stripeMembershipType",
                    Some(membership_type),
                );
            }

            let subscription_status = normalize_non_empty(profile.subscription_status.as_deref());
            if let Some(status) = subscription_status.clone() {
                account.subscription_status = Some(status);
            }
            upsert_cursor_auth_raw_string(
                &mut account,
                "stripeSubscriptionStatus",
                subscription_status,
            );
            upsert_cursor_auth_raw_string(
                &mut account,
                "teamMembershipType",
                normalize_non_empty(profile.team_membership_type.as_deref()),
            );
            upsert_cursor_auth_raw_bool(&mut account, "isTeamMember", profile.is_team_member);
            upsert_cursor_auth_raw_bool(&mut account, "isEnterprise", profile.is_enterprise);

            logger::log_info(&format!(
                "[Cursor Refresh] 订阅信息拉取成功: id={}",
                account.id
            ));
        }
        Ok(None) => {
            logger::log_warn(&format!(
                "[Cursor Refresh] 未获取到订阅信息: id={}",
                account.id
            ));
        }
        Err(err) => {
            logger::log_warn(&format!(
                "[Cursor Refresh] 订阅信息拉取失败: id={}, error={}",
                account.id, err
            ));
        }
    }

    let mut usage_refreshed = false;
    match usage_result {
        Ok(usage) => {
            if let Some(mt) = usage.get("membershipType").and_then(|v| v.as_str()) {
                if !mt.is_empty() {
                    account.membership_type = Some(mt.to_string());
                }
            }
            account.cursor_usage_raw = Some(usage);
            account.quota_query_last_error = None;
            account.quota_query_last_error_at = None;
            usage_refreshed = true;
            logger::log_info(&format!(
                "[Cursor Refresh] API 配额拉取成功: id={}",
                account.id
            ));
        }
        Err(err) => {
            logger::log_warn(&format!(
                "[Cursor Refresh] API 配额拉取失败: id={}, error={}",
                account.id, err
            ));
            account.quota_query_last_error = Some(err);
            account.quota_query_last_error_at = Some(chrono::Utc::now().timestamp_millis());
        }
    }

    match credit_grants_result {
        Ok(credit_grants) => {
            let merged = merge_credit_grants_preserving_history(
                existing.cursor_credit_grants_raw.as_ref(),
                credit_grants,
            );
            account.cursor_credit_grants_raw = Some(merged);
            logger::log_info(&format!(
                "[Cursor Refresh] 赠送额度拉取成功: id={}",
                account.id
            ));
        }
        Err(err) => {
            logger::log_warn(&format!(
                "[Cursor Refresh] 赠送额度拉取失败: id={}, error={}",
                account.id, err
            ));
        }
    }

    let credit_grants_api_valid = account
        .cursor_credit_grants_raw
        .as_ref()
        .map(credit_grants_has_active_remaining)
        .unwrap_or(false);

    if credit_grants_api_valid {
        account.cursor_free_credit_usage_raw = None;
        logger::log_info(&format!(
            "[Cursor Refresh] 赠送额度仍有剩余，跳过 FREE_CREDIT 事件统计: id={}",
            account.id
        ));
    } else {
        match fetch_free_credit_usage_with_client(
            &client,
            &account,
            account.cursor_usage_raw.as_ref(),
        )
        .await
        {
            Ok(Some(snapshot)) => {
                account.cursor_free_credit_usage_raw = Some(snapshot);
                logger::log_info(&format!(
                    "[Cursor Refresh] 赠送额度无剩余/仅历史 peak，已用 FREE_CREDIT 事件汇总: id={}",
                    account.id
                ));
            }
            Ok(None) => {
                account.cursor_free_credit_usage_raw = None;
                logger::log_info(&format!(
                    "[Cursor Refresh] 赠送额度无剩余且无 FREE_CREDIT 事件: id={}",
                    account.id
                ));
            }
            Err(err) => {
                logger::log_warn(&format!(
                    "[Cursor Refresh] FREE_CREDIT 用量拉取失败: id={}, error={}",
                    account.id, err
                ));
            }
        }
    }

    match referral_result {
        Ok(referral) => {
            account.cursor_referral_raw = Some(referral);
            logger::log_info(&format!(
                "[Cursor Refresh] 邀请奖励状态拉取成功: id={}",
                account.id
            ));
        }
        Err(err) => {
            logger::log_warn(&format!(
                "[Cursor Refresh] 邀请奖励状态拉取失败: id={}, error={}",
                account.id, err
            ));
        }
    }

    if is_free_cursor_membership(account.membership_type.as_deref()) {
        match fetch_welcome_back_offer_with_client(&client, &account).await {
            Ok(offer) => {
                let can_activate = welcome_back_can_activate(&offer);
                account.cursor_welcome_back_raw = Some(offer);
                logger::log_info(&format!(
                    "[Cursor Refresh] Comeback 优惠拉取成功: id={}, canActivate={}",
                    account.id, can_activate
                ));
            }
            Err(err) => {
                logger::log_warn(&format!(
                    "[Cursor Refresh] Comeback 优惠拉取失败: id={}, error={}",
                    account.id, err
                ));
            }
        }
    } else if account.cursor_welcome_back_raw.is_some() {
        account.cursor_welcome_back_raw = None;
        logger::log_info(&format!(
            "[Cursor Refresh] 非 FREE 账号，已清除 Comeback 优惠: id={}",
            account.id
        ));
    }

    let sand_access = match sand_access_result {
        Ok(access) => Some(access),
        Err(err) if is_cursor_session_error(&err) => {
            logger::log_warn(&format!(
                "[Cursor Refresh] Grok-Bot 权限因会话失效未更新: id={}, error={}",
                account.id, err
            ));
            None
        }
        Err(err) if err.contains("404") || err.contains("403") => {
            logger::log_info(&format!(
                "[Cursor Refresh] 账号无 Grok-Bot 权限接口: id={}, error={}",
                account.id, err
            ));
            None
        }
        Err(err) => {
            logger::log_warn(&format!(
                "[Cursor Refresh] Grok-Bot 权限拉取失败: id={}, error={}",
                account.id, err
            ));
            None
        }
    };

    apply_cursor_sand_refresh(
        &mut account,
        sand_usage_result,
        sand_access,
        usage_refreshed,
    );

    match latest_usage_event_result {
        Ok(Some(ts_ms)) if ts_ms > 0 => {
            account.cursor_last_usage_event_at = Some(ts_ms / 1000);
            logger::log_info(&format!(
                "[Cursor Refresh] 最近使用时间已更新: id={}, ts={}",
                account.id, ts_ms
            ));
        }
        Ok(Some(_)) | Ok(None) => {
            logger::log_info(&format!(
                "[Cursor Refresh] 未获取到 usage 记录时间，保留上次值: id={}",
                account.id
            ));
        }
        Err(err) if is_cursor_session_error(&err) => {
            logger::log_warn(&format!(
                "[Cursor Refresh] 最近使用时间因会话失效未更新，保留上次状态: id={}, error={}",
                account.id, err
            ));
        }
        Err(err) => {
            logger::log_warn(&format!(
                "[Cursor Refresh] 最近使用时间拉取失败，保留上次状态: id={}, error={}",
                account.id, err
            ));
        }
    }

    let refreshed_at = now_ts();
    if usage_refreshed {
        account.usage_updated_at = Some(refreshed_at);
    }
    account.last_used = refreshed_at;
    let updated = upsert_refreshed_account_record(account)?;
    logger::log_info(&format!(
        "[Cursor Refresh] 刷新完成: id={}, email={}",
        updated.id, updated.email
    ));
    Ok(updated)
}

pub async fn refresh_account_async(account_id: &str) -> Result<CursorAccount, String> {
    let result = refresh_account_async_once(account_id).await;
    if let Err(err) = &result {
        persist_quota_query_error(account_id, err);
    }
    result
}

pub async fn refresh_all_tokens() -> Result<Vec<(String, Result<CursorAccount, String>)>, String> {
    use futures::future::join_all;
    use std::sync::Arc;
    use tokio::sync::Semaphore;

    let active_accounts: Vec<CursorAccount> = list_accounts()
        .into_iter()
        .filter(|account| !is_banned_account(account))
        .collect();

    if active_accounts.is_empty() {
        return Ok(Vec::new());
    }

    logger::log_info(&format!(
        "[Cursor Refresh] 批量刷新开始: accounts={}, maxConcurrent={}",
        active_accounts.len(),
        CURSOR_REFRESH_MAX_CONCURRENT
    ));

    let semaphore = Arc::new(Semaphore::new(CURSOR_REFRESH_MAX_CONCURRENT));
    let tasks: Vec<_> = active_accounts
        .into_iter()
        .map(|account| {
            let account_id = account.id;
            let semaphore = semaphore.clone();
            async move {
                let _permit = semaphore.acquire_owned().await.map_err(|e| {
                    format!("获取 Cursor 刷新并发许可失败: {}", e)
                })?;
                let result = refresh_account_async(&account_id).await;
                Ok::<(String, Result<CursorAccount, String>), String>((account_id, result))
            }
        })
        .collect();

    let mut results = Vec::with_capacity(tasks.len());
    for task in join_all(tasks).await {
        results.push(task?);
    }

    Ok(results)
}

// ---------------------------------------------------------------------------
// Quota alert
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Default)]
struct CursorUsagePercent {
    total_used: Option<i32>,
    auto_used: Option<i32>,
    api_used: Option<i32>,
}

fn clamp_percent(value: f64) -> i32 {
    if !value.is_finite() {
        return 0;
    }
    if value <= 0.0 {
        return 0;
    }
    if value >= 100.0 {
        return 100;
    }
    value.round() as i32
}

fn pick_number(value: Option<&Value>, keys: &[&str]) -> Option<f64> {
    let obj = value?.as_object()?;
    for key in keys {
        let Some(raw) = obj.get(*key) else {
            continue;
        };
        if let Some(n) = raw.as_f64() {
            if n.is_finite() {
                return Some(n);
            }
            continue;
        }
        if let Some(text) = raw.as_str() {
            if let Ok(parsed) = text.trim().parse::<f64>() {
                if parsed.is_finite() {
                    return Some(parsed);
                }
            }
        }
    }
    None
}

fn read_usage_percent(account: &CursorAccount) -> CursorUsagePercent {
    let Some(raw) = account.cursor_usage_raw.as_ref() else {
        return CursorUsagePercent::default();
    };

    let raw_obj = match raw.as_object() {
        Some(value) => value,
        None => return CursorUsagePercent::default(),
    };

    let plan_value = raw_obj
        .get("individualUsage")
        .and_then(|value| value.as_object())
        .and_then(|value| value.get("plan"))
        .or_else(|| {
            raw_obj
                .get("individual_usage")
                .and_then(|value| value.as_object())
                .and_then(|value| value.get("plan"))
        })
        .or_else(|| raw_obj.get("planUsage"))
        .or_else(|| raw_obj.get("plan_usage"));

    let total_direct = pick_number(plan_value, &["totalPercentUsed", "total_percent_used"]);
    let auto_direct = pick_number(plan_value, &["autoPercentUsed", "auto_percent_used"]);
    let api_direct = pick_number(plan_value, &["apiPercentUsed", "api_percent_used"]);

    let used = pick_number(plan_value, &["used", "totalSpend", "total_spend"]);
    let limit = pick_number(plan_value, &["limit"]);
    let total_ratio = match (used, limit) {
        (Some(used_val), Some(limit_val)) if limit_val > 0.0 => {
            Some((used_val / limit_val) * 100.0)
        }
        _ => None,
    };

    CursorUsagePercent {
        total_used: total_direct.or(total_ratio).map(clamp_percent),
        auto_used: auto_direct.map(clamp_percent),
        api_used: api_direct.map(clamp_percent),
    }
}

pub(crate) fn extract_quota_metrics(account: &CursorAccount) -> Vec<(String, i32)> {
    let usage = read_usage_percent(account);
    let mut metrics = Vec::new();

    if let Some(used) = usage.total_used {
        metrics.push(("Total Usage".to_string(), 100 - used.clamp(0, 100)));
    }
    if let Some(used) = usage.auto_used {
        metrics.push(("Auto + Composer".to_string(), 100 - used.clamp(0, 100)));
    }
    if let Some(used) = usage.api_used {
        metrics.push(("API Usage".to_string(), 100 - used.clamp(0, 100)));
    }
    if let Some((used, _)) = cursor_grok_bot_display(account) {
        metrics.push(("Grok-Bot".to_string(), 100 - used.clamp(0, 100)));
    }

    metrics
}

fn average_quota_percentage(metrics: &[(String, i32)]) -> f64 {
    if metrics.is_empty() {
        return 0.0;
    }
    let sum: i32 = metrics.iter().map(|(_, pct)| *pct).sum();
    sum as f64 / metrics.len() as f64
}

fn normalize_quota_alert_threshold(value: i32) -> i32 {
    value.clamp(0, 100)
}

pub(crate) fn resolve_current_account_id(accounts: &[CursorAccount]) -> Option<String> {
    crate::modules::provider_current_state::resolve_existing_current_account_id(
        "cursor",
        accounts.iter().map(|account| account.id.as_str()),
    )
}

fn pick_quota_alert_recommendation(
    accounts: &[CursorAccount],
    current_id: &str,
) -> Option<CursorAccount> {
    let mut candidates: Vec<CursorAccount> = accounts
        .iter()
        .filter(|account| account.id != current_id)
        .filter(|account| !is_banned_account(account))
        .filter(|account| !extract_quota_metrics(account).is_empty())
        .cloned()
        .collect();

    if candidates.is_empty() {
        return None;
    }

    candidates.sort_by(|a, b| {
        let avg_a = average_quota_percentage(&extract_quota_metrics(a));
        let avg_b = average_quota_percentage(&extract_quota_metrics(b));
        avg_b
            .partial_cmp(&avg_a)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.last_used.cmp(&b.last_used))
    });

    candidates.into_iter().next()
}

fn display_email(account: &CursorAccount) -> String {
    let trimmed = account.email.trim();
    if trimmed.is_empty() {
        account.id.clone()
    } else {
        trimmed.to_string()
    }
}

fn build_quota_alert_cooldown_key(account_id: &str, threshold: i32) -> String {
    format!("cursor:{}:{}", account_id, threshold)
}

fn should_emit_quota_alert(cooldown_key: &str, now: i64) -> bool {
    let Ok(mut state) = CURSOR_QUOTA_ALERT_LAST_SENT.lock() else {
        return true;
    };

    if let Some(last_sent) = state.get(cooldown_key) {
        if now - *last_sent < CURSOR_QUOTA_ALERT_COOLDOWN_SECONDS {
            return false;
        }
    }

    state.insert(cooldown_key.to_string(), now);
    true
}

fn clear_quota_alert_cooldown(account_id: &str, threshold: i32) {
    if let Ok(mut state) = CURSOR_QUOTA_ALERT_LAST_SENT.lock() {
        state.remove(&build_quota_alert_cooldown_key(account_id, threshold));
    }
}

pub fn run_quota_alert_if_needed(
) -> Result<Option<crate::modules::account::QuotaAlertPayload>, String> {
    let cfg = crate::modules::config::get_user_config();
    if !cfg.cursor_quota_alert_enabled {
        return Ok(None);
    }

    let threshold = normalize_quota_alert_threshold(cfg.cursor_quota_alert_threshold);
    let accounts = list_accounts();
    let current_id = match resolve_current_account_id(&accounts) {
        Some(id) => id,
        None => return Ok(None),
    };

    let current = match accounts.iter().find(|account| account.id == current_id) {
        Some(account) => account,
        None => return Ok(None),
    };
    if is_banned_account(current) {
        return Ok(None);
    }

    let metrics = extract_quota_metrics(current);
    if metrics.is_empty() {
        clear_quota_alert_cooldown(&current_id, threshold);
        return Ok(None);
    }

    let low_models: Vec<(String, i32)> = metrics
        .into_iter()
        .filter(|(_, pct)| *pct <= threshold)
        .collect();
    if low_models.is_empty() {
        clear_quota_alert_cooldown(&current_id, threshold);
        return Ok(None);
    }

    let now = chrono::Utc::now().timestamp();
    let cooldown_key = build_quota_alert_cooldown_key(&current_id, threshold);
    if !should_emit_quota_alert(&cooldown_key, now) {
        return Ok(None);
    }

    let recommendation = pick_quota_alert_recommendation(&accounts, &current_id);
    let lowest_percentage = low_models.iter().map(|(_, pct)| *pct).min().unwrap_or(0);
    let payload = crate::modules::account::QuotaAlertPayload {
        platform: "cursor".to_string(),
        current_account_id: current_id,
        current_email: display_email(current),
        threshold,
        threshold_display: None,
        lowest_percentage,
        low_models: low_models.into_iter().map(|(name, _)| name).collect(),
        recommended_account_id: recommendation.as_ref().map(|account| account.id.clone()),
        recommended_email: recommendation.as_ref().map(display_email),
        triggered_at: now,
    };

    crate::modules::account::dispatch_quota_alert(&payload);
    Ok(Some(payload))
}

#[cfg(test)]
mod credit_grants_tests {
    use super::{
        build_credit_grants_peak, credit_grants_has_active_remaining,
        credit_grants_has_valid_display_metrics, extract_credit_grants_metrics,
        extract_sand_usage_percent, free_credit_event_cents, is_gift_credit_usage_model,
        merge_credit_grants_preserving_history, sand_access_is_granted,
        sum_free_credit_cents_from_events_page, extract_latest_usage_event_ts_ms,
    };
    use serde_json::json;

    #[test]
    fn extract_metrics_uses_credit_balance_as_remaining() {
        let root = json!({
            "balance": {
                "hasCreditGrants": true,
                "totalCents": "5000",
                "creditBalanceCents": "5000"
            }
        });
        let (total, used, remaining) = extract_credit_grants_metrics(&root);
        assert_eq!(total, Some(5000));
        assert_eq!(remaining, Some(5000));
        assert_eq!(used, Some(0));
    }

    #[test]
    fn peak_preserves_zero_used_when_grant_unused() {
        let fresh = json!({
            "balance": {
                "hasCreditGrants": true,
                "totalCents": "5000",
                "creditBalanceCents": "5000"
            },
            "grants": {}
        });
        let merged = merge_credit_grants_preserving_history(None, fresh);
        let peak = merged.get("peak").expect("peak should exist");
        assert_eq!(peak.get("totalCents").and_then(|v| v.as_i64()), Some(5000));
        assert_eq!(peak.get("usedCents").and_then(|v| v.as_i64()), Some(0));
    }

    #[test]
    fn build_peak_derives_used_from_remaining() {
        let peak = build_credit_grants_peak(5000, None, Some(5000));
        assert_eq!(peak.get("usedCents").and_then(|v| v.as_i64()), Some(0));
        let peak = build_credit_grants_peak(5000, None, Some(1000));
        assert_eq!(peak.get("usedCents").and_then(|v| v.as_i64()), Some(4000));
    }

    #[test]
    fn empty_credit_grants_api_is_invalid_for_display() {
        let empty = json!({ "balance": {}, "grants": {} });
        assert!(!credit_grants_has_valid_display_metrics(&empty));
    }

    #[test]
    fn historical_peak_does_not_count_as_active_remaining() {
        let historical = json!({
            "historical": true,
            "balance": {
                "totalCents": 2500,
                "usedCents": 2500,
                "remainingCents": 0,
                "hasCreditGrants": true,
                "historical": true
            },
            "peak": { "totalCents": 2500, "usedCents": 2500 }
        });
        assert!(credit_grants_has_valid_display_metrics(&historical));
        assert!(!credit_grants_has_active_remaining(&historical));
    }

    #[test]
    fn free_credit_event_cents_reads_float_token_usage() {
        let event = json!({
            "kind": "USAGE_EVENT_KIND_FREE_CREDIT",
            "model": "claude-opus-4-8-thinking-high",
            "usageBasedCosts": "$0.00",
            "tokenUsage": {
                "totalCents": 12.23840045928955
            }
        });
        assert_eq!(free_credit_event_cents(&event), 12);
    }

    #[test]
    fn free_credit_event_cents_falls_back_to_usage_based_costs() {
        let event = json!({
            "kind": "USAGE_EVENT_KIND_FREE_CREDIT",
            "model": "claude-opus-4-8-thinking-high",
            "usageBasedCosts": "$1.25"
        });
        assert_eq!(free_credit_event_cents(&event), 125);
    }

    #[test]
    fn gift_credit_usage_model_excludes_auto_and_composer() {
        assert!(!is_gift_credit_usage_model("default"));
        assert!(!is_gift_credit_usage_model("composer-2.5-fast"));
        assert!(!is_gift_credit_usage_model("composer-2.5"));
        assert!(!is_gift_credit_usage_model("cursor-grok-4.6-xhigh-fast"));
        assert!(!is_gift_credit_usage_model("cursor-grok-4.5-high-fast"));
        assert!(is_gift_credit_usage_model("claude-opus-4-8-thinking-high"));
    }

    #[test]
    fn sum_free_credit_skips_default_and_composer_even_when_kind_matches() {
        let page = json!({
            "usageEventsDisplay": [
                {
                    "kind": "USAGE_EVENT_KIND_FREE_CREDIT",
                    "model": "default",
                    "tokenUsage": { "totalCents": 606 }
                },
                {
                    "kind": "USAGE_EVENT_KIND_FREE_CREDIT",
                    "model": "composer-2.5-fast",
                    "tokenUsage": { "totalCents": 100 }
                },
                {
                    "kind": "USAGE_EVENT_KIND_FREE_CREDIT",
                    "model": "cursor-grok-4.6-xhigh-fast",
                    "tokenUsage": { "totalCents": 1422 }
                },
                {
                    "kind": "USAGE_EVENT_KIND_FREE_CREDIT",
                    "model": "claude-opus-4-8-thinking-high",
                    "tokenUsage": { "totalCents": 2288 }
                }
            ]
        });
        let (sum, count) = sum_free_credit_cents_from_events_page(&page);
        assert_eq!(sum, 2288);
        assert_eq!(count, 1);
    }

    #[test]
    fn latest_usage_event_ts_picks_newest_ms() {
        let root = json!({
            "usageEventsDisplay": [
                { "timestamp": "1787737635000", "model": "composer-2" },
                { "timestamp": "1787742435000", "model": "default" },
                { "timestamp": "1787700000000", "model": "claude-4" }
            ]
        });
        assert_eq!(extract_latest_usage_event_ts_ms(&root), Some(1787742435000));
        assert_eq!(extract_latest_usage_event_ts_ms(&json!({})), None);
    }

    #[test]
    fn sand_usage_percent_reads_float() {
        let root = json!({
            "usagePercent": 45.636,
            "hasAvailableUsage": true,
            "grokPlanLabel": "Grok Bot Plan"
        });
        assert_eq!(extract_sand_usage_percent(&root), Some(45.636));
        assert_eq!(extract_sand_usage_percent(&json!({})), None);
    }

    #[test]
    fn sand_access_granted_detects_state() {
        assert_eq!(
            sand_access_is_granted(&json!({ "state": "SAND_ACCESS_STATE_GRANTED" })),
            Some(true)
        );
        assert_eq!(
            sand_access_is_granted(&json!({
                "state": "SAND_ACCESS_STATE_BLOCKED",
                "blockReason": "SAND_ACCESS_BLOCK_REASON_NONE"
            })),
            Some(false)
        );
        assert_eq!(sand_access_is_granted(&json!({})), None);
    }
}

#[cfg(test)]
mod sand_refresh_tests {
    use super::{apply_cursor_sand_refresh, is_cursor_session_error};
    use crate::models::cursor::CursorAccount;
    use serde_json::json;

    fn account_with_sand(sand: serde_json::Value) -> CursorAccount {
        serde_json::from_value(json!({
            "id": "acc",
            "email": "a@b.c",
            "access_token": "token",
            "created_at": 1,
            "last_used": 1,
            "cursor_sand_usage_raw": sand,
        }))
        .unwrap()
    }

    #[test]
    fn session_error_helper_matches_expired_403() {
        assert!(is_cursor_session_error(
            "Cursor 会话已过期或未认证，请重新导入账号 (HTTP 403)"
        ));
        assert!(!is_cursor_session_error(
            "Cursor dashboard API 返回异常状态码: 404"
        ));
    }

    #[test]
    fn session_403_keeps_previous_sand_usage() {
        let previous = json!({
            "usagePercent": 100.0,
            "nextResetTimestampUtc": "2026-08-29T06:05:00.000Z",
            "access": { "state": "SAND_ACCESS_STATE_GRANTED" }
        });
        let mut account = account_with_sand(previous.clone());
        apply_cursor_sand_refresh(
            &mut account,
            Err("Cursor 会话已过期或未认证，请重新导入账号 (HTTP 403)".into()),
            None,
            false,
        );
        assert_eq!(account.cursor_sand_usage_raw, Some(previous));
    }

    #[test]
    fn blocked_access_with_valid_usage_refresh_clears_sand() {
        let mut account = account_with_sand(json!({ "usagePercent": 12.0 }));
        apply_cursor_sand_refresh(
            &mut account,
            Ok(json!({ "usagePercent": 12.0 })),
            Some(json!({ "state": "SAND_ACCESS_STATE_BLOCKED" })),
            true,
        );
        assert!(account.cursor_sand_usage_raw.is_none());
    }

    #[test]
    fn blocked_access_without_usage_refresh_keeps_sand() {
        let previous = json!({ "usagePercent": 88.0 });
        let mut account = account_with_sand(previous.clone());
        apply_cursor_sand_refresh(
            &mut account,
            Ok(json!({ "usagePercent": 12.0 })),
            Some(json!({ "state": "SAND_ACCESS_STATE_BLOCKED" })),
            false,
        );
        assert_eq!(account.cursor_sand_usage_raw, Some(previous));
    }
}

#[cfg(test)]
mod dashboard_navigation_tests {
    use super::{
        cursor_dashboard_navigation_action, cursor_dashboard_url_stays_in_webview,
        CursorDashboardNavigationAction,
    };

    #[test]
    fn stripe_and_fraud_embed_urls_stay_in_webview() {
        let urls = [
            "https://js.stripe.com/v3/controller-with-preconnect.html",
            "http://js.stripe.com/v3/m-outer-3437.html#url=https%3A%2F%2Fcursor.com%2Fdashboard",
            "https://m.stripe.network/inner.html#url=https%3A%2F%2Fcursor.com%2Fdashboard",
            "https://b.stripecdn.com/stripethirdparty-srv/assets/v32.18/HCaptchaInvisible.html?id=test",
            "https://newassets.hcaptcha.com/captcha/v1/test/static/hcaptcha.html#frame=challenge",
            "https://ri.px-cloud.net/index.html?f=7r5,g34r&v=test",
        ];
        for raw in urls {
            let url = raw.parse().expect("url");
            assert!(
                cursor_dashboard_url_stays_in_webview(&url),
                "expected webview embed: {raw}"
            );
        }
    }

    #[test]
    fn stripe_billing_portal_opens_externally() {
        let billing = "https://billing.stripe.com/p/session/test"
            .parse()
            .expect("billing url");
        assert!(matches!(
            cursor_dashboard_navigation_action(&billing),
            CursorDashboardNavigationAction::OpenInBrowser
        ));
    }

    #[test]
    fn unknown_third_party_urls_are_silently_denied() {
        let random = "https://example.com/track"
            .parse()
            .expect("random url");
        assert!(matches!(
            cursor_dashboard_navigation_action(&random),
            CursorDashboardNavigationAction::DenySilently
        ));
    }
}

#[cfg(test)]
mod auth_sessions_tests {
    use super::{
        cursor_auth_session_revoke_payload, cursor_auth_session_revoke_succeeded,
        cursor_auth_session_revoke_type, is_safe_session_id, parse_cursor_auth_sessions,
    };
    use serde_json::json;

    #[test]
    fn parse_sessions_maps_official_fields() {
        let raw = json!({
            "sessions": [
                {
                    "sessionId": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                    "type": "SESSION_TYPE_WEB",
                    "createdAt": "2026-08-20T05:38:29.000Z",
                    "expiresAt": "2026-10-19T05:38:29.000Z"
                },
                {
                    "sessionId": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                    "type": "SESSION_TYPE_CLIENT",
                    "createdAt": "2026-08-20T05:38:52.000Z",
                    "expiresAt": "2026-10-19T05:38:52.000Z"
                },
                {
                    "sessionId": "",
                    "type": "SESSION_TYPE_WEB",
                    "createdAt": "2026-08-21T00:00:00.000Z"
                }
            ]
        });
        let sessions = parse_cursor_auth_sessions(&raw);
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].session_type, "SESSION_TYPE_WEB");
        assert_eq!(sessions[1].session_type, "SESSION_TYPE_CLIENT");
        assert_eq!(sessions[0].created_at, "2026-08-20T05:38:29.000Z");
        assert_eq!(
            sessions[1].expires_at.as_deref(),
            Some("2026-10-19T05:38:52.000Z")
        );
    }

    #[test]
    fn parse_sessions_maps_numeric_type() {
        let raw = json!({
            "sessions": [
                {
                    "sessionId": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                    "type": 1,
                    "createdAt": "2026-09-08T03:27:01.000Z"
                },
                {
                    "session_id": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                    "type": 2,
                    "created_at": "2026-09-08T03:22:29.000Z"
                }
            ]
        });
        let sessions = parse_cursor_auth_sessions(&raw);
        assert_eq!(sessions[0].session_type, "SESSION_TYPE_WEB");
        assert_eq!(sessions[1].session_type, "SESSION_TYPE_CLIENT");
    }

    #[test]
    fn revoke_payload_requires_matching_type() {
        let web = cursor_auth_session_revoke_payload(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "SESSION_TYPE_WEB",
        )
        .unwrap();
        assert_eq!(
            web,
            json!({
                "sessionId": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "type": "SESSION_TYPE_WEB"
            })
        );
        assert_eq!(
            cursor_auth_session_revoke_type("2").unwrap(),
            "SESSION_TYPE_CLIENT"
        );
        assert!(cursor_auth_session_revoke_payload("id", "").is_err());
        assert!(cursor_auth_session_revoke_succeeded(&json!({ "success": true })));
        assert!(!cursor_auth_session_revoke_succeeded(&json!({})));
    }

    #[test]
    fn session_id_rejects_path_injection() {
        assert!(is_safe_session_id(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        ));
        assert!(!is_safe_session_id("../sessions"));
        assert!(!is_safe_session_id(""));
        assert!(!is_safe_session_id("id with space"));
    }
}

#[cfg(test)]
mod welcome_back_tests {
    use super::{
        extract_checkout_url_from_json, is_free_cursor_membership,
        resolve_welcome_back_checkout_url, welcome_back_can_activate,
    };
    use serde_json::json;

    #[test]
    fn free_membership_only() {
        assert!(is_free_cursor_membership(Some("free")));
        assert!(is_free_cursor_membership(Some("FREE")));
        assert!(!is_free_cursor_membership(Some("pro")));
        assert!(!is_free_cursor_membership(Some("free_trial")));
        assert!(!is_free_cursor_membership(None));
    }

    #[test]
    fn offer_reads_can_activate() {
        assert!(welcome_back_can_activate(&json!({ "canActivate": true })));
        assert!(!welcome_back_can_activate(&json!({ "canActivate": false })));
        assert!(!welcome_back_can_activate(&json!({})));
    }

    #[test]
    fn json_checkout_url() {
        assert_eq!(
            extract_checkout_url_from_json(&json!({
                "checkoutUrl": "https://checkout.stripe.com/c/pay/cs_test"
            })),
            Some("https://checkout.stripe.com/c/pay/cs_test".into())
        );
    }

    #[test]
    fn resolve_prefers_stripe_final_url() {
        let url = resolve_welcome_back_checkout_url(
            "https://checkout.stripe.com/c/pay/cs_test",
            "not-json",
        )
        .unwrap();
        assert!(url.contains("stripe.com"));
    }

    #[test]
    fn resolve_rejects_unrelated_https_url() {
        let err = resolve_welcome_back_checkout_url(
            "https://example.com/track",
            "https://example.com/pay",
        )
        .unwrap_err();
        assert!(err.contains("未获取到"));
    }

    #[test]
    fn resolve_accepts_cursor_landing_page() {
        let url = resolve_welcome_back_checkout_url(
            "https://cursor.com/activate/welcome-back/offer",
            "<html></html>",
        )
        .unwrap();
        assert!(url.contains("welcome-back"));
    }
}

#[cfg(test)]
mod cursor_export_tags_tests {
    use super::{build_cursor_export_item, payload_from_import_value};
    use serde_json::json;

    fn sample_account(tags: Option<Vec<&str>>) -> crate::models::cursor::CursorAccount {
        serde_json::from_value(json!({
            "id": "acc",
            "email": "a@b.c",
            "access_token": "token",
            "created_at": 1,
            "last_used": 1,
            "tags": tags,
        }))
        .unwrap()
    }

    #[test]
    fn export_includes_account_tags() {
        let item = build_cursor_export_item(sample_account(Some(vec!["vip", "work"])));
        assert_eq!(item["tags"], json!(["vip", "work"]));
    }

    #[test]
    fn export_always_writes_tags_field() {
        let item = build_cursor_export_item(sample_account(None));
        assert_eq!(item["tags"], json!([]));
    }

    #[test]
    fn import_payload_reads_tags_when_present() {
        let payload = payload_from_import_value(json!({
            "email": "a@b.c",
            "access_token": "tok",
            "tags": ["vip", " work "]
        }))
        .unwrap();
        assert_eq!(payload.tags, Some(vec!["vip".to_string(), "work".to_string()]));
    }

    #[test]
    fn import_payload_keeps_tags_absent_when_missing() {
        let payload = payload_from_import_value(json!({
            "email": "a@b.c",
            "access_token": "tok"
        }))
        .unwrap();
        assert_eq!(payload.tags, None);
    }
}
