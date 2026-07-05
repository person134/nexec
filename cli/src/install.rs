use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

const RESET: &str = "\x1b[0m";
const GREEN: &str = "\x1b[32m";
const CYAN: &str = "\x1b[36m";
const BOLD_CYAN: &str = "\x1b[1;36m";
const YELLOW: &str = "\x1b[33m";
const BOLD_GREEN: &str = "\x1b[1;32m";
const DIM: &str = "\x1b[2m";

const BOOTLOADER_DIR: &str = "bootloader";
const EFI_FILENAME: &str = "hboot-efi.efi";
const EFI_INSTALL_DIR: &str = "/EFI/hboot/hboot-efi.efi";
const CLI_INSTALL_PATH: &str = "/usr/bin/hboot";
const ESP_CANDIDATES: &[&str] = &["/boot", "/efi", "/boot/efi"];
const SYS_MOUNTS: &str = "/proc/mounts";
const RELEASE_URL: &str = "https://github.com/person134/hboot/releases/latest/download";

macro_rules! cprintln {
    ($color:expr, $($arg:tt)*) => { println!("{}{}{}", $color, format_args!($($arg)*), RESET) };
}

pub struct InstallArgs {
    pub esp_path: Option<String>,
    pub disk: Option<String>,
    pub part: Option<u32>,
    pub efi_path: Option<String>,
    pub no_build: bool,
    pub sign: bool,
    pub sb_key: Option<String>,
    pub sb_cert: Option<String>,
    pub no_config: bool,
}

pub fn install(args: InstallArgs) {
    let InstallArgs {
        esp_path,
        disk,
        part,
        efi_path,
        no_build,
        sign,
        sb_key,
        sb_cert,
        no_config,
    } = args;
    // 1. Build or locate the EFI binary
    let efi_binary = if let Some(path) = &efi_path {
        path.clone()
    } else if no_build {
        eprintln!("error: --efi <path> required when --no-build is set");
        std::process::exit(1);
    } else {
        // Check for a prebuilt EFI next to the CLI binary
        match std::env::current_exe() {
            Ok(exe_path) => {
                let sibling = exe_path.parent().unwrap().join(EFI_FILENAME);
                if sibling.exists() {
                    cprintln!(GREEN, "Using prebuilt EFI: {}", sibling.display());
                    sibling.to_string_lossy().to_string()
                } else if let Some(downloaded) = download_efi_from_release() {
                    cprintln!(GREEN, "Using prebuilt EFI: {}", downloaded.display());
                    downloaded.to_string_lossy().to_string()
                } else {
                    match build_bootloader() {
                        Ok(p) => p,
                        Err(e) => {
                            eprintln!("\x1b[31merror:{} {}", RESET, e);
                            eprintln!("  Either install the UEFI target:");
                            eprintln!("    rustup target add x86_64-unknown-uefi");
                            eprintln!("  Or provide a prebuilt EFI:");
                            eprintln!("    sudo hboot install --efi /path/to/hboot-efi.efi --no-build");
                            std::process::exit(1);
                        }
                    }
                }
            }
            Err(_) => {
                match build_bootloader() {
                    Ok(p) => p,
                    Err(e) => {
                        eprintln!("error: {}", e);
                        std::process::exit(1);
                    }
                }
            }
        }
    };

    if !Path::new(&efi_binary).exists() {
        eprintln!("\x1b[31merror:{} EFI binary not found at {}", RESET, efi_binary);
        std::process::exit(1);
    }

    // 2. Sign the EFI binary for Secure Boot if needed
    let efi_to_install = if should_sign(sign, &sb_key, &sb_cert) {
        match sign_efi(&efi_binary, &sb_key, &sb_cert) {
            Ok(signed) => {
                cprintln!(GREEN, "  Signed EFI for Secure Boot");
                signed
            }
            Err(e) => {
                cprintln!(YELLOW, "warning: Secure Boot signing failed: {}", e);
                cprintln!(YELLOW, "  Continuing with unsigned binary.");
                efi_binary
            }
        }
    } else {
        efi_binary
    };

    // 3. Detect ESP
    let esp = esp_path.unwrap_or_else(|| detect_esp().unwrap_or_else(|| {
        eprintln!("\x1b[31merror:{} could not detect ESP. Specify with --esp", RESET);
        std::process::exit(1);
    }));

    if !Path::new(&esp).is_dir() {
        eprintln!("\x1b[31merror:{} ESP path '{}' is not a directory", RESET, esp);
        std::process::exit(1);
    }

    // 4. Copy EFI binary to ESP
    let install_dir = format!("{}/EFI/hboot", esp.trim_end_matches('/'));
    let install_path = format!("{}/{}", install_dir, EFI_FILENAME);

    cprintln!(CYAN, "Installing to: {}", install_path);
    std::fs::create_dir_all(&install_dir).unwrap_or_else(|e| {
        eprintln!("\x1b[31merror:{} failed to create {}: {}", RESET, install_dir, e);
        std::process::exit(1);
    });
    std::fs::copy(&efi_to_install, &install_path).unwrap_or_else(|e| {
        eprintln!("\x1b[31merror:{} failed to copy {}: {}", RESET, efi_to_install, e);
        std::process::exit(1);
    });
    cprintln!(GREEN, "  Copied {} -> {}", efi_to_install, install_path);

    // 5. Install hboot CLI to system path
    let self_path = std::env::current_exe().unwrap_or_else(|e| {
        cprintln!(YELLOW, "warning: could not determine binary path: {}", e);
        std::process::exit(1);
    });
    let _ = std::fs::remove_file(CLI_INSTALL_PATH);
    std::fs::copy(&self_path, CLI_INSTALL_PATH).unwrap_or_else(|e| {
        eprintln!("\x1b[31merror:{} failed to copy to {}: {} (try running as root)", RESET, CLI_INSTALL_PATH, e);
        std::process::exit(1);
    });
    cprintln!(GREEN, "  Installed CLI to {}", CLI_INSTALL_PATH);

    // 6. Write auto-generated config with detected entries
    if !no_config {
        let (main_conf, entry_files) = crate::config::generate_detected_config(&esp);

        // Print detected entries
        cprintln!(BOLD_CYAN, "Detected:");
        for (name, content) in &entry_files {
            let title = content.lines()
                .find_map(|l| l.strip_prefix("title = "))
                .unwrap_or(name);
            let efi = content.lines()
                .find_map(|l| l.strip_prefix("efi = "))
                .unwrap_or("");
            cprintln!(BOLD_CYAN, "  Entry: {} ({})", title, name);
            cprintln!(DIM, "    efi: {}", efi);
            if let Some(opts) = content.lines().find_map(|l| l.strip_prefix("options = ")) {
                cprintln!(DIM, "    options: {}", opts);
            }
        }

        // Write main config
        let main_conf_path = format!("{}/hboot.conf", install_dir);
        std::fs::write(&main_conf_path, &main_conf).unwrap_or_else(|e| {
            cprintln!(YELLOW, "warning: could not create main config: {}", e);
        });
        cprintln!(GREEN, "  Wrote main config: {}", main_conf_path);

        // Write entry files
        let entries_dir = format!("{}/entries", install_dir);
        std::fs::create_dir_all(&entries_dir).unwrap_or_else(|e| {
            cprintln!(YELLOW, "warning: could not create entries directory: {}", e);
        });
        for (name, content) in &entry_files {
            let entry_path = format!("{}/{}.conf", entries_dir, name);
            std::fs::write(&entry_path, content).unwrap_or_else(|e| {
                cprintln!(YELLOW, "warning: could not write {}: {}", entry_path, e);
            });
            cprintln!(DIM, "  Wrote entry: {}.conf", name);
        }
        cprintln!(YELLOW, "  Edit entries in {}/ to customize.", entries_dir);

        // Also support old single-config format for backwards compat
        let esp_root_conf = format!("{}/hboot.conf", esp.trim_end_matches('/'));
        if Path::new(&esp_root_conf).exists() {
            cprintln!(YELLOW, "  Note: also found config at {}, remove if redundant.", esp_root_conf);
        }
    }
    let (resolved_disk, resolved_part) = resolve_esp_disk_part(&esp, disk, part);
    if !register_efibootmgr(&resolved_disk, resolved_part, EFI_INSTALL_DIR) {
        cprintln!(YELLOW, "  Firmware registration failed, installing fallback entry...");
        let fallback_dir = format!("{}/EFI/BOOT", esp.trim_end_matches('/'));
        let fallback_path = format!("{}/BOOTX64.EFI", fallback_dir);
        std::fs::create_dir_all(&fallback_dir).unwrap_or_else(|e| {
            cprintln!(YELLOW, "warning: could not create {}: {}", fallback_dir, e);
        });
        if let Err(e) = std::fs::copy(&install_path, &fallback_path) {
            cprintln!(YELLOW, "warning: could not copy fallback: {}", e);
        } else {
            cprintln!(GREEN, "  Fallback installed: {} -> {}", install_path, fallback_path);
            cprintln!(YELLOW, "  Your firmware will boot hboot automatically via the UEFI fallback path.");
            cprintln!(YELLOW, "  (No 'hboot' entry in boot menu — it just boots directly.)");
        }
    }

    println!();
    cprintln!(BOLD_GREEN, "hboot installed successfully!");
    if secure_boot_active() {
        cprintln!(GREEN, "  Secure Boot is enabled — the bootloader is signed.");
    }
    cprintln!(YELLOW, "Reboot and select 'hboot' from your UEFI boot menu.");
}

fn should_sign(sign: bool, sb_key: &Option<String>, sb_cert: &Option<String>) -> bool {
    if sb_key.is_some() || sb_cert.is_some() {
        return true;
    }
    if sign {
        return true;
    }
    secure_boot_active()
}

fn secure_boot_active() -> bool {
    Command::new("mokutil")
        .arg("--sb-state")
        .output()
        .ok()
        .map(|o| {
            if o.status.success() {
                let s = String::from_utf8_lossy(&o.stdout);
                s.contains("SecureBoot enabled") || s.contains("enabled")
            } else {
                false
            }
        })
        .unwrap_or(false)
}

fn find_sb_keys(sb_key: &Option<String>, sb_cert: &Option<String>) -> Option<(String, String)> {
    // User-specified keys take priority
    if let (Some(k), Some(c)) = (sb_key, sb_cert) {
        if Path::new(k).exists() && Path::new(c).exists() {
            return Some((k.clone(), c.clone()));
        }
    }

    // Search standard locations
    let candidates = [
        ("/etc/efi-keys/db/db.key", "/etc/efi-keys/db/db.crt"),
        ("/etc/efi-keys/DB.key", "/etc/efi-keys/DB.crt"),
        ("/etc/efi-keys/db.key", "/etc/efi-keys/db.crt"),
        ("/var/lib/shim-signed/mok/mok.key", "/var/lib/shim-signed/mok/mok.crt"),
    ];

    for (key, cert) in &candidates {
        if Path::new(key).exists() && Path::new(cert).exists() {
            return Some((key.to_string(), cert.to_string()));
        }
    }

    None
}

fn sign_efi(efi_path: &str, sb_key: &Option<String>, sb_cert: &Option<String>) -> Result<String, String> {
    let (key, cert) = find_sb_keys(sb_key, sb_cert)
        .ok_or_else(|| "no Secure Boot keys found (install sbsigntools and enroll a MOK)".to_string())?;

    if !Command::new("which").arg("sbsign").output().ok().is_some_and(|o| o.status.success()) {
        return Err("sbsign not found (install sbsigntools)".to_string());
    }

    let signed = format!("{}.signed", efi_path);
    let status = Command::new("sbsign")
        .args(["--key", &key, "--cert", &cert, "--output", &signed, efi_path])
        .status()
        .map_err(|e| format!("failed to run sbsign: {}", e))?;

    if !status.success() {
        return Err("sbsign failed".to_string());
    }

    Ok(signed)
}

fn download_efi_from_release() -> Option<PathBuf> {
    let tmp = std::env::temp_dir().join("hboot_efi_download");
    let _ = std::fs::create_dir_all(&tmp);
    let path = tmp.join(EFI_FILENAME);

    cprintln!(CYAN, "Downloading prebuilt EFI from releases...");
    let status = Command::new("curl")
        .args(["-fsSL", "-o"])
        .arg(&path)
        .arg(&format!("{}/{}", RELEASE_URL, EFI_FILENAME))
        .status()
        .ok()?;

    if status.success() {
        Some(path)
    } else {
        None
    }
}

fn build_bootloader() -> Result<String, String> {
    let project_root = std::env::current_dir().map_err(|e| e.to_string())?;
    let bootloader_dir = project_root.join(BOOTLOADER_DIR);

    // Check if bootloader/ exists relative to cwd
    let build_dir = if bootloader_dir.exists() {
        bootloader_dir
    } else {
        // Maybe we're in the project root or cli/ dir
        let alt = project_root.parent().map(|p| p.join(BOOTLOADER_DIR));
        if let Some(ref alt) = alt {
            if alt.exists() {
                alt.clone()
            } else {
                return Err("cannot find bootloader/ directory".to_string());
            }
        } else {
            return Err("cannot find bootloader/ directory".to_string());
        }
    };

    println!("Building bootloader (x86_64-unknown-uefi)...");

    let status = Command::new("cargo")
        .args(["build", "--target", "x86_64-unknown-uefi", "--release"])
        .current_dir(&build_dir)
        .status()
        .map_err(|e| format!("failed to run cargo: {}", e))?;

    if !status.success() {
        return Err("cargo build failed".to_string());
    }

    let efi_path = build_dir
        .join("target")
        .join("x86_64-unknown-uefi")
        .join("release")
        .join("hboot-efi.efi");

    Ok(efi_path.to_string_lossy().to_string())
}

pub(crate) fn detect_esp() -> Option<String> {
    // Common ESP mount points
    for candidate in ESP_CANDIDATES {
        if Path::new(candidate).is_dir() {
            if let Ok(mounts) = std::fs::read_to_string(SYS_MOUNTS) {
                for line in mounts.lines() {
                    let parts: Vec<&str> = line.split_whitespace().collect();
                    if parts.len() >= 3 && parts[1] == *candidate {
                        let fstype = parts[2];
                        if fstype.contains("fat") || fstype.contains("vfat") {
                            return Some(candidate.to_string());
                        }
                    }
                }
            }
        }
    }
    // Fallback: find any FAT partition
    if let Ok(output) = Command::new("lsblk")
        .args(["-o", "MOUNTPOINT,FSTYPE", "-n", "-r"])
        .output()
    {
        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2 {
                let fstype = parts[1];
                if fstype.contains("fat") || fstype.contains("vfat") {
                    return Some(parts[0].to_string());
                }
            }
        }
    }
    None
}

fn resolve_esp_disk_part(
    esp: &str,
    disk_override: Option<String>,
    part_override: Option<u32>,
) -> (String, u32) {
    if let (Some(d), Some(p)) = (disk_override.as_ref(), part_override) {
        return (d.clone(), p);
    }

    let real_esp = std::path::absolute(esp).unwrap_or_else(|_| Path::new(esp).to_path_buf());

    // Use lsblk to resolve device by mountpoint — handles symlinks, LVM, etc.
    if let Ok(output) = Command::new("lsblk")
        .args(["-pno", "NAME,MAJ:MIN", &real_esp.to_string_lossy()])
        .output()
    {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let first_line = stdout.lines().next().unwrap_or("");
            let dev = first_line.split_whitespace().next().unwrap_or("").trim();
            if !dev.is_empty() {
                // Resolve through symlinks to get the real device
                let real_dev = std::fs::canonicalize(dev).unwrap_or_else(|_| Path::new(dev).to_path_buf());
                let dev_str = real_dev.to_string_lossy();

                // Extract trailing digits as partition number
                let digits: String = dev_str.chars().rev().take_while(|c| c.is_ascii_digit()).collect();
                if !digits.is_empty() {
                    let part_num: u32 = digits.chars().rev().collect::<String>().parse().unwrap_or(1);
                    let dev_base = dev_str.trim_end_matches(&digits.chars().rev().collect::<String>());
                    // Handle nvme and mmc naming: nvme0n1p1 -> nvme0n1, mmcblk0p1 -> mmcblk0
                    let disk_name = dev_base.strip_suffix('p').unwrap_or(dev_base).to_string();
                    return (disk_name, part_num);
                }
            }
        }
    }

    // Fallback: parse /proc/mounts
    let real_esp_str = real_esp.to_string_lossy();
    if let Ok(mounts) = std::fs::read_to_string(SYS_MOUNTS) {
        for line in mounts.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2 && parts[1] == real_esp_str.as_ref() {
                let dev = parts[0];
                let digits: String = dev.chars().rev().take_while(|c| c.is_ascii_digit()).collect();
                if !digits.is_empty() {
                    let part_num: u32 = digits.chars().rev().collect::<String>().parse().unwrap_or(1);
                    let dev_base = dev.trim_end_matches(&digits.chars().rev().collect::<String>());
                    let disk_name = dev_base.strip_suffix('p').unwrap_or(dev_base).to_string();
                    return (disk_name, part_num);
                }
            }
        }
    }

    // Default fallback
    (disk_override.unwrap_or_else(|| "/dev/nvme0n1".to_string()), part_override.unwrap_or(1))
}

fn ensure_efibootmgr() -> bool {
    if Command::new("which").arg("efibootmgr").output().is_ok() {
        return true;
    }

    cprintln!(CYAN, "  Installing efibootmgr...");

    let pm_commands: &[(&str, &[&str])] = &[
        ("pacman", &["-S", "--noconfirm", "efibootmgr"]),
        ("apt", &["install", "-y", "efibootmgr"]),
        ("dnf", &["install", "-y", "efibootmgr"]),
        ("zypper", &["install", "-y", "efibootmgr"]),
        ("apk", &["add", "efibootmgr"]),
        ("yum", &["install", "-y", "efibootmgr"]),
        ("emerge", &["efibootmgr"]),
    ];

    for (pm, args) in pm_commands {
        if Command::new("which").arg(pm).output().is_ok() {
            cprintln!(DIM, "  Detected package manager: {}", pm);
            let status = Command::new(pm)
                .args(*args)
                .status()
                .unwrap_or_else(|e| {
                    eprintln!("\x1b[31merror:{} failed to run {}: {}", RESET, pm, e);
                    std::process::exit(1);
                });
            if status.success() {
                cprintln!(GREEN, "  Installed.");
                return true;
            } else {
                cprintln!(YELLOW, "  {} failed to install efibootmgr.", pm);
                return false;
            }
        }
    }

    cprintln!(YELLOW, "  No supported package manager found. Install efibootmgr manually.");
    false
}

fn register_efibootmgr(disk: &str, part: u32, loader_path: &str) -> bool {
    if !ensure_efibootmgr() {
        return false;
    }

    // Check if hboot is already registered
    if let Ok(output) = Command::new("efibootmgr").output() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines() {
            if line.contains("hboot") {
                if let Some(boot_num) = line.split_whitespace()
                    .find(|s| s.starts_with("Boot"))
                    .and_then(|s| s.trim_start_matches("Boot").trim_end_matches('*').parse::<u32>().ok())
                {
                    cprintln!(YELLOW, "  hboot already registered at Boot{:04X}, skipping.", boot_num);
                    return true;
                }
            }
        }
    }

    cprintln!(CYAN, "Registering with UEFI firmware...");
    let cmd_str = format!(
        "efibootmgr --create --disk {} --part {} --label \"hboot\" --loader \"{}\"",
        disk, part, loader_path
    );
    let output = Command::new("efibootmgr")
        .args([
            "--create",
            "--disk", disk,
            "--part", &part.to_string(),
            "--label", "hboot",
            "--loader", loader_path,
        ])
        .output();

    match output {
        Ok(out) => {
            if out.status.success() {
                cprintln!(GREEN, "  Boot entry created.");
                true
            } else {
                println!("  efibootmgr failed. Common causes:");
                println!("    - Secure Boot is enabled (locks boot entries)");
                println!("    - Firmware does not support efibootmgr");
                println!("    - Missing efivarfs kernel module");
                println!();
                cprintln!(DIM, "  To debug, try this command manually:");
                println!("    sudo {}", cmd_str);
                false
            }
        }
        Err(e) => {
            cprintln!(YELLOW, "  Could not run efibootmgr: {}", e);
            false
        }
    }
}

pub fn status() {
    cprintln!(BOLD_CYAN, "hboot status");
    cprintln!(DIM, "------------");

    let esp = detect_esp();
    match esp {
        Some(ref path) => {
            let installed = format!("{}/EFI/hboot/{}", path.trim_end_matches('/'), EFI_FILENAME);
            if Path::new(&installed).exists() {
                cprintln!(GREEN, "  Installed: yes ({})", installed);
            } else {
                cprintln!(YELLOW, "  Installed: no");
                cprintln!(DIM, "  ESP detected at: {}", path);
                cprintln!(DIM, "  Run 'hboot install' to install.");
            }
        }
        None => {
            cprintln!(YELLOW, "  ESP: not detected");
            cprintln!(DIM, "  Run 'hboot install --esp <path>' to specify manually.");
        }
    }
}

pub fn remove(esp_path: Option<String>, no_efi: bool, all: bool, remove_self: bool) {
    let esp = esp_path.unwrap_or_else(|| detect_esp().unwrap_or_else(|| {
        eprintln!("\x1b[31merror:{} could not detect ESP. Specify with --esp", RESET);
        std::process::exit(1);
    }));

    let mut removed_anything = false;

    // 1. Remove EFI file
    let install_dir = format!("{}/EFI/hboot", esp.trim_end_matches('/'));
    let efi_file = format!("{}/{}", install_dir, EFI_FILENAME);

    if Path::new(&efi_file).exists() {
        std::fs::remove_file(&efi_file).unwrap_or_else(|e| {
            eprintln!("\x1b[31merror:{} failed to remove {}: {}", RESET, efi_file, e);
            std::process::exit(1);
        });
        cprintln!(GREEN, "  Removed: {}", efi_file);
        removed_anything = true;
    } else {
        cprintln!(DIM, "  Not found: {}", efi_file);
    }

    // 2. Remove config if --all
    if all {
        let config_paths = [
            format!("{}/hboot.conf", esp.trim_end_matches('/')),
            format!("{}/EFI/hboot/hboot.conf", esp.trim_end_matches('/')),
        ];
        for cp in &config_paths {
            if Path::new(cp).exists() {
                std::fs::remove_file(cp).unwrap_or_else(|e| {
                    cprintln!(YELLOW, "warning: failed to remove {}: {}", cp, e);
                });
                cprintln!(GREEN, "  Removed: {}", cp);
                removed_anything = true;
            }
        }
        // Remove entries directory
        let entries_dir = format!("{}/EFI/hboot/entries", esp.trim_end_matches('/'));
        if Path::new(&entries_dir).is_dir() {
            if let Ok(dir) = std::fs::read_dir(&entries_dir) {
                for entry in dir.flatten() {
                    let path = entry.path();
                    let _ = std::fs::remove_file(&path);
                }
            }
            let _ = std::fs::remove_dir(&entries_dir);
            cprintln!(GREEN, "  Removed entries directory");
            removed_anything = true;
        }
    }

    // 3. Remove EFI boot entry via efibootmgr
    if !no_efi {
        if let Ok(output) = Command::new("efibootmgr").output() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines() {
                if line.contains("hboot") {
                    if let Some(boot_num) = line.split_whitespace()
                        .find(|s| s.starts_with("Boot"))
                        .and_then(|s| s.trim_start_matches("Boot").trim_end_matches('*').parse::<u32>().ok())
                    {
                        let _ = Command::new("efibootmgr")
                            .args(["-b", &boot_num.to_string(), "-B"])
                            .output();
                        cprintln!(GREEN, "  Removed UEFI boot entry Boot{:04X}", boot_num);
                        removed_anything = true;
                    }
                }
            }
        }
    }

    // 4. Clean up empty directory
    if Path::new(&install_dir).exists() {
        let _ = std::fs::remove_dir(&install_dir);
    }

    // 5. Remove the CLI binary itself
    if remove_self {
        if Path::new(CLI_INSTALL_PATH).exists() {
            std::fs::remove_file(CLI_INSTALL_PATH).unwrap_or_else(|e| {
                cprintln!(YELLOW, "warning: failed to remove {}: {}", CLI_INSTALL_PATH, e);
            });
            cprintln!(GREEN, "  Removed: {}", CLI_INSTALL_PATH);
            removed_anything = true;
        } else {
            cprintln!(DIM, "  Not found: {}", CLI_INSTALL_PATH);
        }
    }

    println!();
    if removed_anything {
        cprintln!(BOLD_GREEN, "hboot removed successfully.");
    }
}
