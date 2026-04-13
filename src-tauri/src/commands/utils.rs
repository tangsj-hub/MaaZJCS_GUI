//! 辅助函数
//!
//! 提供路径处理和其他通用工具函数

use super::types::MaaCallbackEvent;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Emitter};

/// 节点名称 → 人性化描述映射表
/// 格式：(节点名, 描述)
/// 只在 PipelineNode.Succeeded 时打印（节点实际命中执行）
/// 不需要覆盖所有节点，未配置的节点静默跳过
const NODE_LABELS: &[(&str, &str)] = &[
    // ── 副本对战·准备 ──────────────────────────────
    ("DungeonTapScreenArea",            "正在开启副本弹窗"),
    ("DungeonBattlePopupMatchOcr",      "副本弹窗已弹出，开始匹配"),
    ("DungeonBattlePopupNormalOcr",     "选择普通难度"),
    ("DungeonBattlePopupHardOcr",       "选择困难难度"),
    ("DungeonBattlePopupBackIconTmpl",  "副本弹窗：点击返回"),
    ("DungeonBattlePopupBackOcr",       "副本弹窗：点击返回"),
    ("DungeonBattleTapMatch",           "点击匹配，等待对局"),
    ("DungeonBattleTapAcceptOcr",       "接受匹配邀请"),
    ("DungeonBattleUiReadyOcr",         "识别到准备按钮，进入对局"),
    ("DungeonBattleWaitEnd",            "对局进行中，等待结束..."),
    ("DungeonBattleLikeTeam",           "对局结束，正在点赞队友"),
    ("DungeonBattleClaimRewardOcr",     "领取对战奖励"),
    ("DungeonBattleTapTopArea",         "点击返回，退出结算页"),
    ("DungeonBattleMatchLoop",          "匹配轮询中..."),

    // ── 日常副本 ───────────────────────────────────
    ("DungeonTapHomeFirst",             "进入副本首页"),
    ("DungeonUiNotesTabSelectedBiji2Tmpl", "副本笔记已选中"),
    ("DungeonUiNotesTabUnselectedBijiTmpl", "点击副本笔记标签"),
    ("DungeonUiDailyDungeonOcr",        "点击日常副本"),
    ("DungeonTapDungeonName",           "选择副本关卡"),
    ("DungeonSelectDifficulty",         "选择普通难度"),
    ("DungeonTapMatch",                 "点击匹配"),
    ("DungeonTapAcceptOcr",             "接受匹配"),
    ("DungeonUiReadyOcr",               "识别到准备按钮"),
    ("DungeonUiMatchOcr",               "识别到匹配按钮"),
    ("DungeonUiBackIconTmpl",           "点击返回"),
    ("DungeonUiBackOcr",                "点击返回"),
    ("DungeonRecoverTapHome",           "恢复：回到首页"),

    // ── 海之宫副本 ─────────────────────────────────
    ("SeaPalaceTapHomeFirst",           "进入海之宫首页"),
    ("SeaPalaceUiSeaRuinsOcr",          "选择海之宫遗迹"),
    ("SeaPalaceSelectDifficulty",       "选择普通难度"),
    ("SeaPalaceTapMatch",               "点击匹配"),
    ("SeaPalaceTapAcceptOcr",           "接受匹配"),
    ("SeaPalaceUiReadyOcr",             "识别到准备按钮"),

    // ── 副本对战（队长/队员模式）──────────────────
    ("DungeonLeaderMode",               "以队长身份进入"),
    ("DungeonUiStartBattleOcr",         "点击开始战斗"),
    ("DungeonUiInviteOcr",              "发送邀请"),
    ("DungeonLeaderWaitLoop",           "等待队员准备..."),
    ("DungeonMemberMode",               "以队员身份进入"),
    ("DungeonMemberWaitLoop",           "等待队长开始..."),
];

/// 根据命中节点名查找人性化描述
fn node_label(hit_name: &str) -> Option<&'static str> {
    NODE_LABELS
        .iter()
        .find(|(node, _)| *node == hit_name)
        .map(|(_, label)| *label)
}

/// 从 details JSON 字符串中提取指定 key 的字符串值（简单解析，避免外部依赖）
fn extract_str<'a>(details: &'a str, key: &str) -> Option<&'a str> {
    let pattern = format!("\"{}\":", key);
    let start = details.find(pattern.as_str())? + pattern.len();
    let rest = details[start..].trim_start();
    if rest.starts_with('"') {
        let inner = &rest[1..];
        let end = inner.find('"')?;
        Some(&inner[..end])
    } else {
        // 数字等非字符串值，取到 , 或 }
        let end = rest.find([',', '}'])?;
        Some(rest[..end].trim())
    }
}

/// 从 node_details 对象内提取 name 字段（命中的子节点名）
fn extract_hit_node(details: &str) -> Option<&str> {
    let start = details.find("\"node_details\"")?;
    extract_str(&details[start..], "name")
}

/// 从 action_details 对象内提取 action 字段
fn extract_action(details: &str) -> Option<&str> {
    let start = details.find("\"action_details\"")?;
    extract_str(&details[start..], "action")
}

/// 生成终端友好的单行日志
fn format_callback_log(message: &str, details: &str) -> Option<String> {
    match message {
        // ── 控制器 ─────────────────────────────────
        "Controller.Action.Starting" => Some("正在连接设备 ...".to_string()),
        "Controller.Action.Succeeded" => Some("设备连接成功".to_string()),
        "Controller.Action.Failed" => Some("设备连接失败！".to_string()),

        // ── 资源加载 ───────────────────────────────
        "Resource.Loading.Starting" => {
            let name = extract_str(details, "path")
                .and_then(|p| p.rsplit(['/', '\\']).next())
                .unwrap_or("资源");
            Some(format!("正在加载资源: {}", name))
        }
        "Resource.Loading.Succeeded" => Some("资源加载成功".to_string()),
        "Resource.Loading.Failed" => {
            let name = extract_str(details, "path").unwrap_or("未知资源");
            Some(format!("资源加载失败: {}", name))
        }

        // ── 任务级别 ───────────────────────────────
        "Tasker.Task.Starting" => {
            let entry = extract_str(details, "entry").unwrap_or("未知任务");
            Some(format!("任务开始: {}", entry))
        }
        "Tasker.Task.Succeeded" => {
            let entry = extract_str(details, "entry").unwrap_or("未知任务");
            Some(format!("任务完成: {}", entry))
        }
        "Tasker.Task.Failed" => {
            let entry = extract_str(details, "entry").unwrap_or("未知任务");
            Some(format!("任务失败: {}", entry))
        }

        // ── 节点级别：只打印有人性化标签的关键节点 ──
        "Node.PipelineNode.Succeeded" => {
            let hit = extract_hit_node(details).unwrap_or("");
            if let Some(label) = node_label(hit) {
                // 跳过纯轮询节点（避免刷屏）
                let is_loop = hit.ends_with("Loop") || hit.ends_with("WaitLoop");
                if !is_loop {
                    return Some(format!("{}", label));
                }
            }
            None
        }
        "Node.PipelineNode.Failed" => {
            let name = extract_str(details, "name").unwrap_or("");
            let hit = extract_hit_node(details).unwrap_or("");
            // 只打印有标签的节点失败，或者动作不是 StopTask 的失败（StopTask 是正常结束）
            let action = extract_action(details).unwrap_or("");
            if action == "StopTask" {
                return None;
            }
            if let Some(label) = node_label(hit).or_else(|| node_label(name)) {
                return Some(format!("⚠ {} 失败", label));
            }
            None
        }

        _ => None,
    }
}

/// 发送回调事件到前端
pub fn emit_callback_event<S: Into<String>>(app: &AppHandle, message: S, details: S) {
    let message = message.into();
    let details = details.into();

    if let Some(log_line) = format_callback_log(&message, &details) {
        log::info!("[MAA] {}", log_line);
    }

    let event = MaaCallbackEvent { message, details };
    if let Err(e) = app.emit("maa-callback", event) {
        log::error!("Failed to emit maa-callback: {}", e);
    }
}

/// 获取应用数据目录
/// - macOS: ~/Library/Application Support/MXU/
/// - Windows/Linux: exe 所在目录（保持便携式部署）
pub fn get_app_data_dir() -> Result<PathBuf, String> {
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var("HOME").map_err(|_| "无法获取 HOME 环境变量".to_string())?;
        let path = PathBuf::from(home)
            .join("Library")
            .join("Application Support")
            .join("MXU");
        Ok(path)
    }

    #[cfg(not(target_os = "macos"))]
    {
        // Windows/Linux 保持便携式，使用 exe 所在目录
        get_exe_directory()
    }
}

/// 规范化路径：移除冗余的 `.`、处理 `..`、统一分隔符
/// 使用 Path::components() 解析，不需要路径实际存在
pub fn normalize_path(path: &str) -> PathBuf {
    use std::path::{Component, Path};

    let path = Path::new(path);
    let mut components = Vec::new();

    for component in path.components() {
        match component {
            // 跳过当前目录标记 "."
            Component::CurDir => {}
            // 处理父目录 ".."：如果栈顶是普通目录则弹出，否则保留
            Component::ParentDir => {
                if matches!(components.last(), Some(Component::Normal(_))) {
                    components.pop();
                } else {
                    components.push(component);
                }
            }
            // 保留其他组件（Prefix、RootDir、Normal）
            _ => components.push(component),
        }
    }

    // 重建路径
    components.into_iter().collect()
}

/// 获取日志目录（应用数据目录下的 debug 子目录）
pub fn get_logs_dir() -> PathBuf {
    get_app_data_dir()
        .unwrap_or_else(|_| {
            // 回退到 exe 目录
            let exe_path = std::env::current_exe().unwrap_or_default();
            exe_path
                .parent()
                .unwrap_or(std::path::Path::new("."))
                .to_path_buf()
        })
        .join("debug")
}

fn get_project_root_override() -> Result<Option<PathBuf>, String> {
    match std::env::var_os("MXU_PROJECT_ROOT") {
        Some(value) => {
            let path = PathBuf::from(value);
            if path.exists() {
                Ok(Some(path))
            } else {
                Err(format!(
                    "MXU_PROJECT_ROOT 指向的目录不存在: {}",
                    path.display()
                ))
            }
        }
        None => Ok(None),
    }
}

fn find_dev_project_root(exe_dir: &Path) -> Option<PathBuf> {
    exe_dir.ancestors().find_map(|dir| {
        let has_interface = dir.join("interface.json").exists();
        let has_maafw = dir.join("maafw").exists();
        if has_interface && has_maafw {
            Some(dir.to_path_buf())
        } else {
            None
        }
    })
}

/// 获取 exe 所在目录路径（内部使用）
pub fn get_exe_directory() -> Result<PathBuf, String> {
    if let Some(project_root) = get_project_root_override()? {
        return Ok(project_root);
    }

    let exe_path = std::env::current_exe().map_err(|e| format!("获取 exe 路径失败: {}", e))?;
    let exe_dir = exe_path
        .parent()
        .map(|p| p.to_path_buf())
        .ok_or_else(|| "无法获取 exe 所在目录".to_string())?;

    if exe_dir.join("interface.json").exists() {
        return Ok(exe_dir);
    }

    if let Some(project_root) = find_dev_project_root(&exe_dir) {
        log::info!(
            "Detected development project root from exe directory: {}",
            project_root.display()
        );
        return Ok(project_root);
    }

    Ok(exe_dir)
}

/// 获取可执行文件所在目录下的 maafw 子目录（资源根，不区分 bin 布局）
pub fn get_maafw_dir() -> Result<PathBuf, String> {
    Ok(get_exe_directory()?.join("maafw"))
}

/// 实际包含 MaaFramework 动态库的目录（用于 LoadLibrary / SetDllDirectoryW）。
///
/// 官方文档将 `bin` 内文件铺在 `maafw` 根下；CMake `install` 常见布局为 `maafw/bin/*.dll`。
#[cfg(windows)]
fn maa_framework_lib_filename() -> &'static str {
    "MaaFramework.dll"
}

#[cfg(target_os = "macos")]
fn maa_framework_lib_filename() -> &'static str {
    "libMaaFramework.dylib"
}

#[cfg(target_os = "linux")]
fn maa_framework_lib_filename() -> &'static str {
    "libMaaFramework.so"
}

pub fn get_maafw_lib_dir() -> Result<PathBuf, String> {
    let base = get_maafw_dir()?;
    let name = maa_framework_lib_filename();
    if base.join(name).exists() {
        return Ok(base);
    }
    let in_bin = base.join("bin").join(name);
    if in_bin.exists() {
        return Ok(base.join("bin"));
    }
    Ok(base)
}

/// 构建 User-Agent 字符串
pub fn build_user_agent() -> String {
    let version = env!("CARGO_PKG_VERSION");
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    let tauri_version = tauri::VERSION;
    format!("MXU/{} ({}; {}) Tauri/{}", version, os, arch, tauri_version)
}
