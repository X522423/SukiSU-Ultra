use anyhow::{Result, anyhow};
use notify::{Watcher, RecursiveMode};
use std::path::Path;
use std::fs;
use std::ffi::OsStr;
use std::process::Command;

pub const KPM_DIR: &str = "/data/adb/kpm";
pub const KPMMGR_PATH: &str = "/data/adb/ksu/bin/kpmmgr";

// 获取KPM版本
pub fn get_kpm_version() -> Result<String> {
    let output = Command::new(KPMMGR_PATH)
        .arg("version")
        .output()
        .map_err(|e| anyhow!("Failed to execute kpmmgr: {}", e))?;
    
    if output.status.success() {
        let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok(version)
    } else {
        let error = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(anyhow!("Error getting KPM version: {}", error))
    }
}

// 检查KPM是否已配置
pub fn check_kpm_configured() -> bool {
    Path::new(KPMMGR_PATH).exists() && is_executable(KPMMGR_PATH)
}

// 检查文件是否可执行
fn is_executable(path: &str) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(metadata) = fs::metadata(path) {
            return metadata.permissions().mode() & 0o111 != 0;
        }
    }
    false
}

// 创建确保 KPM 目录存在
pub fn ensure_kpm_dir() -> Result<()> {
    // 检查KPM版本和配置状态
    let kpm_version_result = get_kpm_version();
    let is_kpm_configured = check_kpm_configured();
    
    match kpm_version_result {
        Ok(version) if !version.to_lowercase().starts_with("error") => {
            log::info!("KPM version {} detected, proceeding with directory creation", version);
            if !Path::new(KPM_DIR).exists() {
                fs::create_dir_all(KPM_DIR)?;
                log::info!("Created KPM directory: {}", KPM_DIR);
            }
            Ok(())
        },
        Ok(version) => {
            log::warn!("KPM version check returned an error: {}", version);
            Err(anyhow!("KPM not properly configured: {}", version))
        },
        Err(e) => {
            log::error!("KPM version check failed: {}", e);
            Err(anyhow!("KPM not properly configured: {}", e))
        }
    }
}

pub fn start_kpm_watcher() -> Result<()> {
    ensure_kpm_dir()?;

    // 检查是否处于安全模式
    if crate::utils::is_safe_mode() {
        log::warn!("The system is in safe mode and is deleting all KPM modules...");
        if let Err(e) = remove_all_kpms() {
            log::error!("Error deleting all KPM modules: {}", e);
        }
        return Ok(());
    }

    let mut watcher = notify::recommended_watcher(|res| {
        match res {
            Ok(event) => handle_kpm_event(event),
            Err(e) => log::error!("monitoring error: {:?}", e),
        }
    })?;

    watcher.watch(Path::new(KPM_DIR), RecursiveMode::NonRecursive)?;
    Ok(())
}

// 处理 KPM 事件
pub fn handle_kpm_event(event: notify::Event) {
    match event.kind {
        notify::EventKind::Create(_) => handle_create_event(event.paths),
        notify::EventKind::Remove(_) => handle_remove_event(event.paths),
        notify::EventKind::Modify(_) => handle_modify_event(event.paths),
        _ => {}
    }
}

fn handle_create_event(paths: Vec<std::path::PathBuf>) {
    for path in paths {
        if path.extension() == Some(OsStr::new("kpm")) {
            if let Err(e) = load_kpm(&path) {
                log::warn!("Failed to load {}: {}", path.display(), e);
            }
        }
    }
}

fn handle_remove_event(paths: Vec<std::path::PathBuf>) {
    for path in paths {
        if let Some(name) = path.file_stem().and_then(|s| s.to_str()) {
            if let Err(e) = unload_kpm(name) {
                log::warn!("Failed to unload {}: {}", name, e);
            }
            if let Err(e) = fs::remove_file(&path) {
                log::error!("Failed to delete file: {}: {}", path.display(), e);
            }
        }
    }
}

fn handle_modify_event(paths: Vec<std::path::PathBuf>) {
    for path in paths {
        log::info!("Modified file: {}", path.display());
    }
}

// 加载 KPM 模块
pub fn load_kpm(path: &Path) -> Result<()> {
    let path_str = path.to_str().ok_or_else(|| anyhow!("Invalid path: {}", path.display()))?;
    let status = std::process::Command::new(KPMMGR_PATH)
        .args(["load", path_str, ""])
        .status()?;

    if status.success() {
        log::info!("Loaded KPM: {}", path.display());
    }
    Ok(())
}

// 卸载 KPM 模块并尝试删除对应文件
pub fn unload_kpm(name: &str) -> Result<()> {
    let status = std::process::Command::new(KPMMGR_PATH)
        .args(["unload", name])
        .status()
        .map_err(|e| anyhow!("Failed to execute kpmmgr: {}", e))?;

    if status.success() {
        let kpm_path = find_kpm_file(name)?;
        if let Some(path) = kpm_path {
            fs::remove_file(&path)
                .map_err(|e| anyhow!("Failed to delete KPM file: {}: {}", path.display(), e))?;
            log::info!("Deleted KPM file: {}", path.display());
        }

        log::info!("Successfully unloaded KPM: {}", name);
    } else {
        log::warn!("KPM unloading may have failed: {}", name);
    }

    Ok(())
}

// 通过名称查找 KPM 文件
fn find_kpm_file(name: &str) -> Result<Option<std::path::PathBuf>> {
    let kpm_dir = Path::new(KPM_DIR);
    if !kpm_dir.exists() {
        return Ok(None);
    }

    for entry in fs::read_dir(kpm_dir)? {
        let path = entry?.path();
        if let Some(file_name) = path.file_stem() {
            if let Some(file_name_str) = file_name.to_str() {
                if file_name_str == name && path.extension() == Some(OsStr::new("kpm")) {
                    return Ok(Some(path));
                }
            }
        }
    }
    Ok(None)
}

// 安全模式下删除所有 KPM 模块
pub fn remove_all_kpms() -> Result<()> {
    ensure_kpm_dir()?;

    for entry in fs::read_dir(KPM_DIR)? {
        let path = entry?.path();
        if path.extension().is_some_and(|ext| ext == "kpm") {
            if let Some(name) = path.file_stem() {
                if let Err(e) = unload_kpm(name.to_string_lossy().as_ref()) {
                    log::error!("Failed to remove KPM: {}", e);
                }
                if let Err(e) = fs::remove_file(&path) {
                    log::error!("Failed to delete file: {}: {}", path.display(), e);
                }
            }
        }
    }
    Ok(())
}

// 加载 KPM 模块
pub fn load_kpm_modules() -> Result<()> {
    ensure_kpm_dir()?;

    for entry in std::fs::read_dir(KPM_DIR)? {
        let path = entry?.path();
        if let Some(file_name) = path.file_stem() {
            if let Some(file_name_str) = file_name.to_str() {
                if file_name_str.is_empty() {
                    log::warn!("Invalid KPM file name: {}", path.display());
                    continue;
                }
            }
        }
    
        if path.extension().is_some_and(|ext| ext == "kpm") {
            match load_kpm(&path) {
                Ok(()) => log::info!("Successfully loaded KPM module: {}", path.display()),
                Err(e) => log::warn!("Failed to load KPM module {}: {}", path.display(), e),
            }
        }
    }

    Ok(())
}