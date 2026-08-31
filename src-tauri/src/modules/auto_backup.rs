use std::collections::HashMap;
use std::fs;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::{json, Map, Value};

use crate::modules::config::{self, UserConfig};
use crate::modules::{
    account, claude_account, codebuddy_account, codebuddy_cn_account, codex_account,
    codex_instance, codex_wakeup, cursor_account, cursor_instance, gemini_account, gemini_instance,
    github_copilot_account, github_copilot_instance, group_settings, instance, kiro_account,
    kiro_instance, logger, qoder_account, qoder_instance, trae_account, trae_instance,
    windsurf_account, windsurf_instance, workbuddy_account, workbuddy_instance, zed_account,
};

const DATA_TRANSFER_SCHEMA: &str = "cockpit-tools.data-transfer";
const DATA_TRANSFER_VERSION: u32 = 1;
const ACCOUNT_TRANSFER_SCHEMA: &str = "cockpit-tools.account-transfer";
const ACCOUNT_TRANSFER_VERSION: u32 = 1;
const AUTO_BACKUP_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
pub const STARTUP_DELAY: Duration = Duration::from_secs(20);
pub const POLL_INTERVAL: Duration = Duration::from_secs(15 * 60);

const BACKUP_PLATFORMS: &[&str] = &[
    "antigravity",
    "antigravity_ide",
    "codex",
    "claude_manager",
    "zed",
    "github-copilot",
    "windsurf",
    "kiro",
    "cursor",
    "gemini",
    "codebuddy",
    "codebuddy_cn",
    "qoder",
    "trae",
    "workbuddy",
];

const INSTANCE_PLATFORMS: &[&str] = &[
    "antigravity",
    "codex",
    "github-copilot",
    "windsurf",
    "kiro",
    "cursor",
    "gemini",
    "codebuddy",
    "codebuddy_cn",
    "qoder",
    "trae",
    "workbuddy",
];

static CYCLE_RUNNING: AtomicBool = AtomicBool::new(false);

pub struct CycleGuard;

impl Drop for CycleGuard {
    fn drop(&mut self) {
        CYCLE_RUNNING.store(false, Ordering::SeqCst);
    }
}

pub fn try_begin_cycle() -> Result<CycleGuard, String> {
    if CYCLE_RUNNING
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Err("自动备份正在执行中".to_string());
    }
    Ok(CycleGuard)
}

#[derive(Debug, Clone, Serialize)]
pub struct AutoBackupCycleResult {
    pub ran: bool,
    pub file_name: Option<String>,
    pub path: Option<String>,
    pub executed_at: Option<String>,
    pub deleted_files: Vec<String>,
    pub skipped_reason: Option<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct PlatformExport {
    pub platform: String,
    pub account_count: u64,
    pub exported_data: Value,
    pub refs: HashMap<String, Value>,
    pub warning: Option<String>,
}

pub fn parse_last_backup_at(value: Option<&str>) -> Option<DateTime<Utc>> {
    let raw = value?.trim();
    if raw.is_empty() {
        return None;
    }
    DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|parsed| parsed.with_timezone(&Utc))
}

pub fn is_auto_backup_due(
    enabled: bool,
    include_accounts: bool,
    include_config: bool,
    last_backup_at: Option<&str>,
    now: DateTime<Utc>,
) -> bool {
    if !enabled || (!include_accounts && !include_config) {
        return false;
    }
    match parse_last_backup_at(last_backup_at) {
        None => true,
        Some(last) => {
            now.signed_duration_since(last)
                >= chrono::Duration::from_std(AUTO_BACKUP_INTERVAL)
                    .unwrap_or(chrono::Duration::hours(24))
        }
    }
}

pub fn backup_mode_label(include_accounts: bool, include_config: bool) -> &'static str {
    match (include_accounts, include_config) {
        (true, true) => "full",
        (true, false) => "accounts",
        (false, true) => "config",
        (false, false) => "full",
    }
}

pub fn format_backup_file_name(
    trigger: &str,
    include_accounts: bool,
    include_config: bool,
    executed_at: DateTime<Utc>,
) -> String {
    let local = executed_at.with_timezone(&chrono::Local);
    format!(
        "cockpit_{}_backup_{}_{}.json",
        trigger,
        backup_mode_label(include_accounts, include_config),
        local.format("%Y-%m-%d_%H-%M-%S")
    )
}

fn parse_export_json(raw: &str, fallback_count: usize) -> Result<(u64, Value), String> {
    let value: Value =
        serde_json::from_str(raw).map_err(|err| format!("invalid_export_json: {err}"))?;
    let count = match &value {
        Value::Array(items) => items.len(),
        Value::Null => 0,
        _ => fallback_count.max(1),
    };
    Ok((count as u64, value))
}

fn export_from_ids<F>(ids: Vec<String>, export_fn: F) -> Result<(u64, Value), String>
where
    F: FnOnce(&[String]) -> Result<String, String>,
{
    if ids.is_empty() {
        return Ok((0, json!([])));
    }
    let raw = export_fn(&ids)?;
    parse_export_json(&raw, ids.len())
}

fn antigravity_export_json(ids: &[String]) -> Result<String, String> {
    let mut accounts = Vec::new();
    if ids.is_empty() {
        accounts = account::list_accounts()?;
    } else {
        for id in ids {
            if let Ok(item) = account::load_account(id) {
                accounts.push(item);
            }
        }
    }

    #[derive(Serialize)]
    struct SimpleAccount {
        email: String,
        refresh_token: String,
        #[serde(default)]
        tags: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        notes: Option<String>,
    }

    let simplified: Vec<SimpleAccount> = accounts
        .into_iter()
        .map(|item| SimpleAccount {
            email: item.email,
            refresh_token: item.token.refresh_token,
            tags: item.tags,
            notes: item.notes,
        })
        .collect();
    serde_json::to_string_pretty(&simplified).map_err(|err| format!("序列化失败: {err}"))
}

fn email_ref(platform: &str, id: &str, email: &str) -> (String, Value) {
    (
        id.to_string(),
        json!({
            "platform": platform,
            "email": email,
        }),
    )
}

fn export_platform(platform: &str) -> PlatformExport {
    let result = match platform {
        "antigravity" | "antigravity_ide" => (|| {
            let accounts = account::list_accounts()?;
            let refs = accounts
                .iter()
                .filter(|item| !item.email.trim().is_empty())
                .map(|item| email_ref(platform, &item.id, &item.email))
                .collect();
            let ids = accounts.into_iter().map(|item| item.id).collect::<Vec<_>>();
            let (account_count, exported_data) = export_from_ids(ids, antigravity_export_json)?;
            Ok((account_count, exported_data, refs))
        })(),
        "codex" => (|| {
            let accounts = codex_account::list_accounts();
            let refs = accounts
                .iter()
                .filter(|item| !item.email.trim().is_empty())
                .map(|item| email_ref(platform, &item.id, &item.email))
                .collect();
            let ids = accounts.into_iter().map(|item| item.id).collect::<Vec<_>>();
            let (account_count, exported_data) =
                export_from_ids(ids, |ids| codex_account::export_accounts(ids))?;
            Ok((account_count, exported_data, refs))
        })(),
        "claude_manager" => (|| {
            let accounts = claude_account::list_accounts();
            let refs = accounts
                .iter()
                .filter(|item| !item.email.trim().is_empty())
                .map(|item| email_ref(platform, &item.id, &item.email))
                .collect();
            let ids = accounts.into_iter().map(|item| item.id).collect::<Vec<_>>();
            let (account_count, exported_data) =
                export_from_ids(ids, |ids| claude_account::export_accounts(ids))?;
            Ok((account_count, exported_data, refs))
        })(),
        "zed" => (|| {
            let accounts = zed_account::list_accounts();
            let refs = accounts
                .iter()
                .map(|item| {
                    (
                        item.id.clone(),
                        json!({
                            "platform": platform,
                            "userId": item.user_id,
                            "githubLogin": item.github_login,
                        }),
                    )
                })
                .collect();
            let ids = accounts.into_iter().map(|item| item.id).collect::<Vec<_>>();
            let (account_count, exported_data) =
                export_from_ids(ids, |ids| zed_account::export_accounts(ids))?;
            Ok((account_count, exported_data, refs))
        })(),
        "github-copilot" => (|| {
            let accounts = github_copilot_account::list_accounts();
            let refs = accounts
                .iter()
                .map(|item| {
                    let mut obj = Map::new();
                    obj.insert("platform".into(), json!(platform));
                    obj.insert("githubLogin".into(), json!(item.github_login));
                    obj.insert("githubId".into(), json!(item.github_id));
                    if let Some(email) = item
                        .github_email
                        .as_deref()
                        .filter(|value| !value.trim().is_empty())
                    {
                        obj.insert("email".into(), json!(email));
                    }
                    (item.id.clone(), Value::Object(obj))
                })
                .collect();
            let ids = accounts.into_iter().map(|item| item.id).collect::<Vec<_>>();
            let (account_count, exported_data) =
                export_from_ids(ids, |ids| github_copilot_account::export_accounts(ids))?;
            Ok((account_count, exported_data, refs))
        })(),
        "windsurf" => (|| {
            let accounts = windsurf_account::list_accounts();
            let refs = accounts
                .iter()
                .map(|item| {
                    let mut obj = Map::new();
                    obj.insert("platform".into(), json!(platform));
                    obj.insert("githubLogin".into(), json!(item.github_login));
                    obj.insert("githubId".into(), json!(item.github_id));
                    if let Some(email) = item
                        .github_email
                        .as_deref()
                        .filter(|value| !value.trim().is_empty())
                    {
                        obj.insert("email".into(), json!(email));
                    }
                    (item.id.clone(), Value::Object(obj))
                })
                .collect();
            let ids = accounts.into_iter().map(|item| item.id).collect::<Vec<_>>();
            let (account_count, exported_data) =
                export_from_ids(ids, |ids| windsurf_account::export_accounts(ids))?;
            Ok((account_count, exported_data, refs))
        })(),
        "kiro" => (|| {
            let accounts = kiro_account::list_accounts();
            let refs = accounts
                .iter()
                .filter(|item| !item.email.trim().is_empty())
                .map(|item| email_ref(platform, &item.id, &item.email))
                .collect();
            let ids = accounts.into_iter().map(|item| item.id).collect::<Vec<_>>();
            let (account_count, exported_data) =
                export_from_ids(ids, |ids| kiro_account::export_accounts(ids))?;
            Ok((account_count, exported_data, refs))
        })(),
        "cursor" => (|| {
            let accounts = cursor_account::list_accounts();
            let refs = accounts
                .iter()
                .filter(|item| !item.email.trim().is_empty())
                .map(|item| email_ref(platform, &item.id, &item.email))
                .collect();
            let ids = accounts.into_iter().map(|item| item.id).collect::<Vec<_>>();
            let (account_count, exported_data) =
                export_from_ids(ids, |ids| cursor_account::export_accounts(ids))?;
            Ok((account_count, exported_data, refs))
        })(),
        "gemini" => (|| {
            let accounts = gemini_account::list_accounts();
            let refs = accounts
                .iter()
                .filter(|item| !item.email.trim().is_empty())
                .map(|item| email_ref(platform, &item.id, &item.email))
                .collect();
            let ids = accounts.into_iter().map(|item| item.id).collect::<Vec<_>>();
            let (account_count, exported_data) =
                export_from_ids(ids, |ids| gemini_account::export_accounts(ids))?;
            Ok((account_count, exported_data, refs))
        })(),
        "codebuddy" => (|| {
            let accounts = codebuddy_account::list_accounts();
            let refs = accounts
                .iter()
                .filter(|item| !item.email.trim().is_empty())
                .map(|item| email_ref(platform, &item.id, &item.email))
                .collect();
            let ids = accounts.into_iter().map(|item| item.id).collect::<Vec<_>>();
            let (account_count, exported_data) =
                export_from_ids(ids, |ids| codebuddy_account::export_accounts(ids))?;
            Ok((account_count, exported_data, refs))
        })(),
        "codebuddy_cn" => (|| {
            let accounts = codebuddy_cn_account::list_accounts();
            let refs = accounts
                .iter()
                .filter(|item| !item.email.trim().is_empty())
                .map(|item| email_ref(platform, &item.id, &item.email))
                .collect();
            let ids = accounts.into_iter().map(|item| item.id).collect::<Vec<_>>();
            let (account_count, exported_data) =
                export_from_ids(ids, |ids| codebuddy_cn_account::export_accounts(ids))?;
            Ok((account_count, exported_data, refs))
        })(),
        "qoder" => (|| {
            let accounts = qoder_account::list_accounts();
            let refs = accounts
                .iter()
                .filter(|item| !item.email.trim().is_empty())
                .map(|item| email_ref(platform, &item.id, &item.email))
                .collect();
            let ids = accounts.into_iter().map(|item| item.id).collect::<Vec<_>>();
            let (account_count, exported_data) =
                export_from_ids(ids, |ids| qoder_account::export_accounts(ids))?;
            Ok((account_count, exported_data, refs))
        })(),
        "trae" => (|| {
            let accounts = trae_account::list_accounts();
            let refs = accounts
                .iter()
                .filter(|item| !item.email.trim().is_empty())
                .map(|item| email_ref(platform, &item.id, &item.email))
                .collect();
            let ids = accounts.into_iter().map(|item| item.id).collect::<Vec<_>>();
            let (account_count, exported_data) =
                export_from_ids(ids, |ids| trae_account::export_accounts(ids))?;
            Ok((account_count, exported_data, refs))
        })(),
        "workbuddy" => (|| {
            let accounts = workbuddy_account::list_accounts();
            let refs = accounts
                .iter()
                .filter(|item| !item.email.trim().is_empty())
                .map(|item| email_ref(platform, &item.id, &item.email))
                .collect();
            let ids = accounts.into_iter().map(|item| item.id).collect::<Vec<_>>();
            let (account_count, exported_data) =
                export_from_ids(ids, |ids| workbuddy_account::export_accounts(ids))?;
            Ok((account_count, exported_data, refs))
        })(),
        other => Err(format!("unsupported_platform:{other}")),
    };

    match result {
        Ok((account_count, exported_data, refs)) => PlatformExport {
            platform: platform.to_string(),
            account_count,
            exported_data,
            refs,
            warning: None,
        },
        Err(error) => {
            logger::log_warn(&format!(
                "[AutoBackup] 平台导出失败，已跳过: platform={platform}, error={error}"
            ));
            PlatformExport {
                platform: platform.to_string(),
                account_count: 0,
                exported_data: json!([]),
                refs: HashMap::new(),
                warning: Some(format!("{platform}: {error}")),
            }
        }
    }
}

fn read_data_json(file_name: &str) -> Option<Value> {
    let path = account::get_data_dir().ok()?.join(file_name);
    if !path.exists() {
        return None;
    }
    let content = fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

fn refs_for_ids(ids: &[Value], lookup: &HashMap<String, Value>) -> Vec<Value> {
    ids.iter()
        .filter_map(|item| item.as_str())
        .filter_map(|id| lookup.get(id).cloned())
        .collect()
}

fn remap_account_groups(raw: Option<Value>, lookup: &HashMap<String, Value>) -> Value {
    let Some(Value::Array(groups)) = raw else {
        return json!([]);
    };
    Value::Array(
        groups
            .into_iter()
            .map(|group| {
                let mut next = match group {
                    Value::Object(map) => map,
                    other => {
                        let mut map = Map::new();
                        map.insert("raw".into(), other);
                        map
                    }
                };
                let ids = next
                    .remove("accountIds")
                    .or_else(|| next.remove("account_ids"))
                    .and_then(|value| match value {
                        Value::Array(items) => Some(items),
                        _ => None,
                    })
                    .unwrap_or_default();
                next.insert(
                    "accountRefs".into(),
                    Value::Array(refs_for_ids(&ids, lookup)),
                );
                Value::Object(next)
            })
            .collect(),
    )
}

fn load_instance_store(platform: &str) -> Result<crate::models::InstanceStore, String> {
    match platform {
        "antigravity" => instance::load_instance_store(),
        "codex" => codex_instance::load_instance_store(),
        "github-copilot" => github_copilot_instance::load_instance_store(),
        "windsurf" => windsurf_instance::load_instance_store(),
        "kiro" => kiro_instance::load_instance_store(),
        "cursor" => cursor_instance::load_instance_store(),
        "gemini" => gemini_instance::load_instance_store(),
        "codebuddy" => crate::modules::codebuddy_instance::load_instance_store(),
        "codebuddy_cn" => crate::modules::codebuddy_cn_instance::load_instance_store(),
        "qoder" => qoder_instance::load_instance_store(),
        "trae" => trae_instance::load_instance_store(),
        "workbuddy" => workbuddy_instance::load_instance_store(),
        other => Err(format!("unsupported_instance_platform:{other}")),
    }
}

fn export_instance_store(platform: &str, lookup: &HashMap<String, Value>) -> Option<Value> {
    let store = match load_instance_store(platform) {
        Ok(store) => store,
        Err(error) => {
            logger::log_warn(&format!(
                "[AutoBackup] 读取实例配置失败，已跳过: platform={platform}, error={error}"
            ));
            return None;
        }
    };

    Some(json!({
        "defaultSettings": {
            "bindAccountRef": store
                .default_settings
                .bind_account_id
                .as_deref()
                .and_then(|id| lookup.get(id).cloned()),
            "extraArgs": store.default_settings.extra_args,
            "launchMode": store.default_settings.launch_mode,
            "followLocalAccount": store.default_settings.follow_local_account,
        },
        "instances": store
            .instances
            .iter()
            .map(|item| json!({
                "id": item.id,
                "name": item.name,
                "userDataDir": item.user_data_dir,
                "workingDir": item.working_dir,
                "extraArgs": item.extra_args,
                "bindAccountRef": item
                    .bind_account_id
                    .as_deref()
                    .and_then(|id| lookup.get(id).cloned()),
                "launchMode": item.launch_mode,
                "createdAt": item.created_at,
            }))
            .collect::<Vec<_>>(),
    }))
}

fn export_antigravity_wakeup(lookup: &HashMap<String, Value>) -> Value {
    let mut root = read_data_json("wakeup_tasks.json")
        .unwrap_or_else(|| json!({ "enabled": false, "tasks": [] }));
    if let Some(tasks) = root.get_mut("tasks").and_then(Value::as_array_mut) {
        for task in tasks {
            let Some(schedule) = task.get_mut("schedule").and_then(Value::as_object_mut) else {
                continue;
            };
            let ids = schedule
                .remove("selectedAccounts")
                .or_else(|| schedule.remove("selected_accounts"))
                .and_then(|value| match value {
                    Value::Array(items) => Some(items),
                    _ => None,
                })
                .unwrap_or_default();
            schedule.insert(
                "selectedAccountRefs".into(),
                Value::Array(refs_for_ids(&ids, lookup)),
            );
        }
    }
    if root.get("official_ls_version_mode").is_none() && root.get("officialLsVersionMode").is_none()
    {
        root.as_object_mut()
            .map(|map| map.insert("official_ls_version_mode".into(), json!("auto")));
    }
    root
}

fn export_codex_wakeup(lookup: &HashMap<String, Value>) -> Value {
    let state = match codex_wakeup::load_state() {
        Ok(state) => state,
        Err(error) => {
            logger::log_warn(&format!(
                "[AutoBackup] 读取 Codex 唤醒配置失败，已使用空配置: error={error}"
            ));
            codex_wakeup::CodexWakeupState::default()
        }
    };

    let tasks = state
        .tasks
        .iter()
        .map(|task| {
            let mut value = serde_json::to_value(task).unwrap_or_else(|_| json!({}));
            if let Some(obj) = value.as_object_mut() {
                let ids = task
                    .account_ids
                    .iter()
                    .cloned()
                    .map(Value::String)
                    .collect::<Vec<_>>();
                obj.insert(
                    "account_refs".into(),
                    Value::Array(refs_for_ids(&ids, lookup)),
                );
            }
            value
        })
        .collect::<Vec<_>>();

    json!({
        "enabled": state.enabled,
        "model_presets": state.model_presets,
        "runtime": {},
        "tasks": tasks,
    })
}

fn export_user_config(config: &UserConfig, lookup: &HashMap<String, Value>) -> Value {
    let mut value = serde_json::to_value(config).unwrap_or_else(|_| json!({}));
    if let Some(obj) = value.as_object_mut() {
        obj.remove("webdav_sync_password");
        let antigravity_ids = obj
            .remove("auto_switch_selected_account_ids")
            .and_then(|value| match value {
                Value::Array(items) => Some(items),
                _ => None,
            })
            .unwrap_or_default();
        let codex_ids = obj
            .remove("codex_auto_switch_selected_account_ids")
            .and_then(|value| match value {
                Value::Array(items) => Some(items),
                _ => None,
            })
            .unwrap_or_default();
        obj.insert(
            "auto_switch_selected_account_refs".into(),
            Value::Array(refs_for_ids(&antigravity_ids, lookup)),
        );
        obj.insert(
            "codex_auto_switch_selected_account_refs".into(),
            Value::Array(refs_for_ids(&codex_ids, lookup)),
        );
    }
    value
}

fn build_accounts_bundle(
    platforms: &[PlatformExport],
    exported_at: &str,
) -> (Value, HashMap<String, Value>, Vec<String>) {
    let mut platform_map = Map::new();
    let mut refs = HashMap::new();
    let mut warnings = Vec::new();
    let mut account_count = 0u64;

    for item in platforms {
        if let Some(warning) = &item.warning {
            warnings.push(warning.clone());
        }
        account_count += item.account_count;
        refs.extend(item.refs.clone());
        platform_map.insert(
            item.platform.clone(),
            json!({
                "account_count": item.account_count,
                "exported_data": item.exported_data,
            }),
        );
    }

    (
        json!({
            "schema": ACCOUNT_TRANSFER_SCHEMA,
            "version": ACCOUNT_TRANSFER_VERSION,
            "exported_at": exported_at,
            "summary": {
                "platform_count": platforms.len(),
                "account_count": account_count,
            },
            "platforms": platform_map,
        }),
        refs,
        warnings,
    )
}

fn build_config_bundle(config: &UserConfig, lookup: &HashMap<String, Value>) -> Value {
    let mut instance_stores = Map::new();
    for platform in INSTANCE_PLATFORMS {
        if let Some(store) = export_instance_store(platform, lookup) {
            instance_stores.insert((*platform).to_string(), store);
        }
    }

    json!({
        "user_config": export_user_config(config, lookup),
        "group_settings": group_settings::load_group_settings(),
        "account_groups": remap_account_groups(read_data_json("account_groups.json"), lookup),
        "codex_account_groups": remap_account_groups(read_data_json("codex_account_groups.json"), lookup),
        "codex_model_providers": read_data_json("codex_model_providers.json").unwrap_or_else(|| json!([])),
        "instance_stores": instance_stores,
        "antigravity_wakeup": export_antigravity_wakeup(lookup),
        "codex_wakeup": export_codex_wakeup(lookup),
        "current_account_refresh_minutes": {},
    })
}

pub fn build_backup_bundle(
    include_accounts: bool,
    include_config: bool,
    exported_at: DateTime<Utc>,
) -> Result<(String, Vec<String>), String> {
    if !include_accounts && !include_config {
        return Err("transfer_selection_required".to_string());
    }

    let exported_at_text = exported_at.to_rfc3339();
    let mut warnings = Vec::new();
    let mut bundle = json!({
        "schema": DATA_TRANSFER_SCHEMA,
        "version": DATA_TRANSFER_VERSION,
        "exported_at": exported_at_text,
        "sections": {
            "accounts": include_accounts,
            "config": include_config,
        },
    });

    let platforms: Vec<PlatformExport> = BACKUP_PLATFORMS
        .iter()
        .map(|platform| export_platform(platform))
        .collect();
    let (accounts_bundle, refs, account_warnings) =
        build_accounts_bundle(&platforms, &exported_at_text);
    warnings.extend(account_warnings);

    if include_accounts {
        bundle
            .as_object_mut()
            .ok_or_else(|| "backup_bundle_not_object".to_string())?
            .insert("accounts".into(), accounts_bundle);
    }

    if include_config {
        let config = config::get_user_config();
        bundle
            .as_object_mut()
            .ok_or_else(|| "backup_bundle_not_object".to_string())?
            .insert("config".into(), build_config_bundle(&config, &refs));
    }

    let content = serde_json::to_string_pretty(&bundle)
        .map_err(|err| format!("序列化自动备份失败: {err}"))?;
    Ok((content, warnings))
}

pub fn skipped(reason: &str) -> AutoBackupCycleResult {
    AutoBackupCycleResult {
        ran: false,
        file_name: None,
        path: None,
        executed_at: None,
        deleted_files: Vec::new(),
        skipped_reason: Some(reason.to_string()),
        warnings: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn due_when_never_backed_up() {
        let now = Utc.with_ymd_and_hms(2026, 8, 30, 12, 0, 0).unwrap();
        assert!(is_auto_backup_due(true, true, true, None, now));
    }

    #[test]
    fn not_due_within_24_hours() {
        let now = Utc.with_ymd_and_hms(2026, 8, 30, 12, 0, 0).unwrap();
        assert!(!is_auto_backup_due(
            true,
            true,
            true,
            Some("2026-08-30T10:00:00Z"),
            now
        ));
    }

    #[test]
    fn due_after_24_hours() {
        let now = Utc.with_ymd_and_hms(2026, 8, 30, 12, 0, 0).unwrap();
        assert!(is_auto_backup_due(
            true,
            true,
            true,
            Some("2026-06-25T09:39:39.580Z"),
            now
        ));
    }

    #[test]
    fn disabled_or_empty_selection_is_never_due() {
        let now = Utc.with_ymd_and_hms(2026, 8, 30, 12, 0, 0).unwrap();
        assert!(!is_auto_backup_due(false, true, true, None, now));
        assert!(!is_auto_backup_due(true, false, false, None, now));
    }

    #[test]
    fn file_name_uses_local_timestamp_and_mode() {
        let executed_at = Utc.with_ymd_and_hms(2026, 6, 25, 9, 39, 39).unwrap();
        let name = format_backup_file_name("auto", true, true, executed_at);
        assert!(name.starts_with("cockpit_auto_backup_full_"));
        assert!(name.ends_with(".json"));
        assert_eq!(backup_mode_label(true, false), "accounts");
        assert_eq!(backup_mode_label(false, true), "config");
    }

    #[test]
    fn parse_export_json_keeps_single_object_and_counts_arrays() {
        let (count, value) = parse_export_json(r#"{"email":"a@b.com"}"#, 1).unwrap();
        assert_eq!(count, 1);
        assert!(value.is_object());

        let (count, value) =
            parse_export_json(r#"[{"email":"a@b.com"},{"email":"c@d.com"}]"#, 2).unwrap();
        assert_eq!(count, 2);
        assert_eq!(value.as_array().map(Vec::len), Some(2));
    }

    #[test]
    fn empty_ids_export_as_empty_array() {
        let (count, value) = export_from_ids(Vec::new(), |_| Ok("[]".to_string())).unwrap();
        assert_eq!(count, 0);
        assert_eq!(value, json!([]));
    }

    #[test]
    fn failed_platform_is_isolated_in_bundle() {
        let platforms = vec![
            PlatformExport {
                platform: "cursor".into(),
                account_count: 2,
                exported_data: json!([{"email":"a@b.com"},{"email":"c@d.com"}]),
                refs: HashMap::new(),
                warning: None,
            },
            PlatformExport {
                platform: "codex".into(),
                account_count: 0,
                exported_data: json!([]),
                refs: HashMap::new(),
                warning: Some("codex: boom".into()),
            },
        ];
        let (bundle, _, warnings) = build_accounts_bundle(&platforms, "2026-08-30T00:00:00Z");
        assert_eq!(bundle["summary"]["account_count"], 2);
        assert_eq!(bundle["platforms"]["cursor"]["account_count"], 2);
        assert_eq!(bundle["platforms"]["codex"]["account_count"], 0);
        assert_eq!(warnings, vec!["codex: boom".to_string()]);
        assert_eq!(bundle["schema"], ACCOUNT_TRANSFER_SCHEMA);
    }

    #[test]
    fn remap_groups_converts_account_ids_to_refs() {
        let mut lookup = HashMap::new();
        lookup.insert(
            "acc-1".into(),
            json!({"platform":"cursor","email":"a@b.com"}),
        );
        let remapped = remap_account_groups(
            Some(json!([{
                "id": "g1",
                "name": "VIP",
                "accountIds": ["acc-1", "missing"]
            }])),
            &lookup,
        );
        assert_eq!(
            remapped[0]["accountRefs"],
            json!([{"platform":"cursor","email":"a@b.com"}])
        );
        assert!(remapped[0].get("accountIds").is_none());
    }
}
