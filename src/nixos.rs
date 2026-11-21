//! NixOS 专属：`/etc/asusd/*.ron` 可能由 `environment.etc`（`services.asusd.*Config`）
//! 声明。asusd 运行时写回会把 symlink 替换成普通文件，本次运行有效；但下一次
//! boot 或 `nixos-rebuild switch` 时，激活脚本按系统代重新生成 `/etc`，改动丢失。
//!
//! 这里只做识别、提示与导出，不修改 NixOS 配置树：持久化仍由用户通过
//! `services.asusd` 声明并 rebuild 完成。

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::Serialize;

pub const ETC_ASUSD: &str = "/etc/asusd";
pub const STATIC_ASUSD: &str = "/etc/static/asusd";

/// 由 `services.asusd` 声明、因此运行时改动不持久的单个文件。
#[derive(Debug, Clone, Serialize)]
pub struct ManagedFile {
    pub name: String,
    /// 运行时路径：/etc/asusd/<name>
    pub path: PathBuf,
    /// NixOS 声明版本：/etc/static/asusd/<name>
    pub declared: PathBuf,
    /// 声明版本指向的 /nix/store 文件
    pub store: Option<PathBuf>,
    /// 运行时文件是否已被普通文件替换（asusd 写入后的形态）
    pub overwritten: bool,
    /// 运行时内容与声明内容是否不同
    pub diverged: bool,
}

impl ManagedFile {
    /// 对应的 `services.asusd` 选项；未知文件返回 None。
    pub fn option(&self) -> Option<String> {
        match self.name.as_str() {
            "asusd.ron" => Some("asusdConfig".into()),
            "fan_curves.ron" => Some("fanCurvesConfig".into()),
            "profile.ron" => Some("profileConfig".into()),
            name if name.starts_with("aura_") && name.ends_with(".ron") => {
                let id = &name["aura_".len()..name.len() - ".ron".len()];
                Some(format!("auraConfigs.\"{id}\""))
            }
            _ => None,
        }
    }

    /// 中文摘要，用于 doctor / status。
    pub fn summary(&self) -> &'static str {
        match (self.overwritten, self.diverged) {
            (false, false) => "与 NixOS 声明一致",
            (false, true) => "与声明不一致",
            (true, false) => "运行时为普通文件，内容与声明一致",
            (true, true) => "运行时已覆盖，重启或 switch 后回滚",
        }
    }
}

/// asusd.ron 中与用户最相关、最容易“设置了却没生效”的两个字段。
#[derive(Debug, Clone, Serialize)]
pub struct DeclaredProfile {
    pub field: &'static str,
    pub declared: Option<String>,
    pub runtime: Option<String>,
}

/// NixOS 与 asusd 托管状态。
#[derive(Debug, Clone, Serialize)]
pub struct Status {
    /// 是否运行在 NixOS 上
    pub nixos: bool,
    /// 判断依据（/etc/NIXOS、os-release、/run/current-system）
    pub reason: String,
    /// 是否存在由 NixOS 声明的 asusd 文件；有则运行时改动不可持久化
    pub managed: bool,
    pub files: Vec<ManagedFile>,
    /// AC / 电池默认模式的声明值与运行时值
    pub profiles: Vec<DeclaredProfile>,
}

impl Status {
    /// 附加到操作结果后的短提示。
    pub fn note(&self) -> Option<&'static str> {
        self.managed.then_some("NixOS：重启后回滚")
    }
}

pub fn detect() -> Status {
    let (nixos, reason) = detect_nixos();
    let files = if nixos {
        managed_files(Path::new(ETC_ASUSD), Path::new(STATIC_ASUSD))
    } else {
        Vec::new()
    };
    Status {
        nixos,
        reason,
        managed: !files.is_empty(),
        files,
        profiles: if nixos { profiles() } else { Vec::new() },
    }
}

/// 对比 /etc/static/asusd/asusd.ron（声明）与 /etc/asusd/asusd.ron（运行时）。
fn profiles() -> Vec<DeclaredProfile> {
    let declared = fs::read_to_string(Path::new(STATIC_ASUSD).join("asusd.ron")).ok();
    let runtime = fs::read_to_string(Path::new(ETC_ASUSD).join("asusd.ron")).ok();
    ["platform_profile_on_ac", "platform_profile_on_battery"]
        .into_iter()
        .filter_map(|field| {
            let decl = declared.as_deref().and_then(|text| ron_value(text, field));
            let run = runtime.as_deref().and_then(|text| ron_value(text, field));
            (decl.is_some() || run.is_some()).then_some(DeclaredProfile {
                field,
                declared: decl,
                runtime: run,
            })
        })
        .collect()
}

/// 从 RON 文本里取顶层的 `key: value,`。
fn ron_value(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let rest = line.trim().strip_prefix(key)?.trim_start();
        let rest = rest.strip_prefix(':')?.trim();
        Some(rest.trim_end_matches(',').trim().to_string())
    })
}

fn detect_nixos() -> (bool, String) {
    if Path::new("/etc/NIXOS").exists() {
        return (true, "/etc/NIXOS".into());
    }
    if let Some(id) = os_release_id()
        && id == "nixos"
    {
        return (true, "os-release ID=nixos".into());
    }
    if Path::new("/run/current-system").exists() {
        return (true, "/run/current-system".into());
    }
    (false, "未检测到 NixOS 标记".into())
}

fn os_release_id() -> Option<String> {
    let text = fs::read_to_string("/etc/os-release").ok()?;
    text.lines().find_map(|line| {
        let (key, value) = line.split_once('=')?;
        (key == "ID").then(|| value.trim().trim_matches('"').to_string())
    })
}

/// 枚举 /etc/static/asusd 下已声明的文件，并与运行时 /etc/asusd 对比。
fn managed_files(etc: &Path, static_etc: &Path) -> Vec<ManagedFile> {
    let mut names = vec![
        "asusd.ron".to_string(),
        "fan_curves.ron".to_string(),
        "profile.ron".to_string(),
    ];
    if let Ok(entries) = fs::read_dir(static_etc) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("aura_") && name.ends_with(".ron") && !names.contains(&name) {
                names.push(name);
            }
        }
    }

    names
        .into_iter()
        .filter_map(|name| inspect(etc, static_etc, &name))
        .collect()
}

fn inspect(etc: &Path, static_etc: &Path, name: &str) -> Option<ManagedFile> {
    let declared = static_etc.join(name);
    // 没有声明版本，说明这个文件不由 NixOS 管理，运行时写入可以持久。
    if !declared.exists() {
        return None;
    }
    let path = etc.join(name);
    let store = fs::canonicalize(&declared)
        .ok()
        .filter(|target| target.starts_with("/nix/store"));
    let overwritten = fs::symlink_metadata(&path)
        .map(|meta| !meta.file_type().is_symlink())
        .unwrap_or(false);
    let diverged = match (fs::read(&path), fs::read(&declared)) {
        (Ok(runtime), Ok(decl)) => runtime != decl,
        _ => false,
    };
    Some(ManagedFile {
        name: name.to_string(),
        path,
        declared,
        store,
        overwritten,
        diverged,
    })
}

/// 把当前运行时 asusd 配置导出为可直接放进 NixOS 配置的模块片段。
pub fn export_module(status: &Status) -> Result<String> {
    if !status.nixos {
        anyhow::bail!("当前不是 NixOS，/etc/asusd 的运行时写入本身就能持久化");
    }

    let mut entries = Vec::new();
    for file in &status.files {
        let Some(option) = file.option() else {
            continue;
        };
        let Ok(text) = fs::read_to_string(&file.path) else {
            continue;
        };
        entries.push((option, text));
    }
    if entries.is_empty() {
        anyhow::bail!("没有可导出的 asusd 运行时配置（/etc/asusd 下没有已声明的文件）");
    }

    let mut out = String::new();
    out.push_str("# 由 rogtui 根据当前运行时配置生成。\n");
    out.push_str("# 放进本机 NixOS 配置后执行：sudo nixos-rebuild switch\n");
    out.push_str("{ ... }:\n{\n  services.asusd = {\n");
    for (option, text) in &entries {
        out.push_str(&format!("    {option}.text = ''\n"));
        for line in text.lines() {
            out.push_str("      ");
            out.push_str(&line.replace("${", "''${"));
            out.push('\n');
        }
        out.push_str("    '';\n");
    }
    out.push_str("  };\n}\n");
    Ok(out)
}

/// 在完成一次会写入 /etc/asusd 的操作后，把 NixOS 回滚提示打到 stderr。
/// 非 NixOS 或文件未托管时什么也不做。
pub fn warn_runtime_only() {
    let status = detect();
    if let Some(note) = status.note() {
        eprintln!(
            "提示：{note}——/etc/asusd 由 NixOS 声明；运行 `rogtui nixos export` 生成可固化片段"
        );
    }
}

/// 供 TUI 概览页使用的一句话说明。
pub fn banner(status: &Status) -> Option<&'static str> {
    status
        .managed
        .then_some("asusd 配置由 NixOS 声明，改动重启回滚；`rogtui nixos export` 可固化")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(tag: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("rogtui-nixos-{tag}-{stamp}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn inspect_detects_declared_and_diverged() {
        let etc = temp_dir("etc");
        let static_etc = temp_dir("static");
        fs::create_dir_all(etc.join("asusd")).unwrap();
        fs::create_dir_all(static_etc.join("asusd")).unwrap();
        fs::write(static_etc.join("asusd/asusd.ron"), "declared = true\n").unwrap();
        fs::write(etc.join("asusd/asusd.ron"), "declared = false\n").unwrap();

        let file = inspect(&etc.join("asusd"), &static_etc.join("asusd"), "asusd.ron").unwrap();
        assert!(file.overwritten);
        assert!(file.diverged);
        assert_eq!(file.summary(), "运行时已覆盖，重启或 switch 后回滚");

        fs::remove_dir_all(&etc).ok();
        fs::remove_dir_all(&static_etc).ok();
    }

    #[test]
    fn undeclared_file_is_not_managed() {
        let etc = temp_dir("etc2");
        let static_etc = temp_dir("static2");
        fs::create_dir_all(etc.join("asusd")).unwrap();
        fs::create_dir_all(static_etc.join("asusd")).unwrap();
        fs::write(etc.join("asusd/asusd.ron"), "x = 1\n").unwrap();

        assert!(inspect(&etc.join("asusd"), &static_etc.join("asusd"), "asusd.ron").is_none());

        fs::remove_dir_all(&etc).ok();
        fs::remove_dir_all(&static_etc).ok();
    }

    #[test]
    fn parses_ron_profile_values() {
        let text =
            "  platform_profile_on_ac: Balanced,\n  platform_profile_on_battery: Performance,\n";
        assert_eq!(
            ron_value(text, "platform_profile_on_ac").as_deref(),
            Some("Balanced")
        );
        assert_eq!(
            ron_value(text, "platform_profile_on_battery").as_deref(),
            Some("Performance")
        );
        assert_eq!(ron_value(text, "change_platform_profile_on_ac"), None);
    }

    #[test]
    fn options_match_nixos_module() {
        let file = ManagedFile {
            name: "aura_tuf.ron".into(),
            path: PathBuf::new(),
            declared: PathBuf::new(),
            store: None,
            overwritten: false,
            diverged: false,
        };
        assert_eq!(file.option().as_deref(), Some("auraConfigs.\"tuf\""));
    }
}
