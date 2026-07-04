use std::path::Path;
use std::process::Command;

const VERSION: &str = env!("CARGO_PKG_VERSION");
const RELEASE_URL: &str = "https://github.com/person134/hboot/releases/latest/download";

fn detect_esp_for_update() -> Option<String> {
    for candidate in &["/boot", "/efi", "/boot/efi"] {
        if Path::new(candidate).is_dir() {
            if let Ok(mounts) = std::fs::read_to_string("/proc/mounts") {
                for line in mounts.lines() {
                    let parts: Vec<&str> = line.split_whitespace().collect();
                    if parts.len() >= 3 && parts[1] == *candidate {
                        if parts[2].contains("fat") || parts[2].contains("vfat") {
                            return Some(candidate.to_string());
                        }
                    }
                }
            }
        }
    }
    None
}

pub fn update() {
    println!("hboot update v{}", VERSION);
    println!("Fetching latest release...");

    let uid = Command::new("id").arg("-u").output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    if uid != "0" {
        eprintln!("error: update requires root (need to write to /usr/bin and ESP)");
        std::process::exit(1);
    }

    let tmp = std::env::temp_dir().join("hboot_update");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap_or_else(|e| {
        eprintln!("error: failed to create temp dir: {}", e);
        std::process::exit(1);
    });

    let cli_path = tmp.join("hboot");
    let efi_path = tmp.join("hboot-efi.efi");

    println!("  Downloading hboot...");
    let status = Command::new("curl")
        .args(["-fsSL", "-o"])
        .arg(&cli_path)
        .arg(&format!("{}/hboot", RELEASE_URL))
        .status()
        .unwrap_or_else(|e| {
            eprintln!("error: failed to run curl: {}", e);
            eprintln!("  Is curl installed?");
            std::process::exit(1);
        });
    if !status.success() {
        eprintln!("error: failed to download hboot from releases");
        std::process::exit(1);
    }

    let _ = std::fs::set_permissions(&cli_path, std::os::unix::fs::PermissionsExt::from_mode(0o755));

    println!("  Downloading hboot-efi.efi...");
    let status = Command::new("curl")
        .args(["-fsSL", "-o"])
        .arg(&efi_path)
        .arg(&format!("{}/hboot-efi.efi", RELEASE_URL))
        .status()
        .unwrap_or_else(|e| {
            eprintln!("error: failed to run curl: {}", e);
            std::process::exit(1);
        });
    if !status.success() {
        eprintln!("error: failed to download hboot-efi.efi from releases");
        std::process::exit(1);
    }

    println!("  Installing EFI to ESP...");
    let status = Command::new(&cli_path)
        .args(["install", "--no-config", "--no-build", "--efi"])
        .arg(&efi_path)
        .status()
        .unwrap_or_else(|e| {
            eprintln!("error: failed to run installer: {}", e);
            std::process::exit(1);
        });
    if !status.success() {
        eprintln!("error: installer failed");
        std::process::exit(1);
    }

    println!("  Ensuring recovery_timeout in config...");
    let esp = detect_esp_for_update().unwrap_or_default();
    let config_paths = [
        format!("{}/EFI/hboot/hboot.conf", esp),
        format!("{}/hboot.conf", esp),
    ];
    for path in &config_paths {
        if !Path::new(path).exists() {
            continue;
        }
        let content = std::fs::read_to_string(path).unwrap_or_default();
        if content.contains("recovery_timeout") {
            break;
        }
        let updated = format!("{}\nrecovery_timeout = 5\n", content.trim_end());
        std::fs::write(path, &updated).unwrap_or_else(|e| {
            eprintln!("warning: failed to update {}: {}", path, e);
        });
        println!("    Added recovery_timeout = 5 to {}", path);
        break;
    }

    println!("  Updating /usr/bin/hboot...");
    std::fs::copy(&cli_path, "/usr/bin/hboot").unwrap_or_else(|e| {
        eprintln!("error: failed to copy hboot to /usr/bin: {}", e);
        std::process::exit(1);
    });
    let _ = std::fs::set_permissions("/usr/bin/hboot", std::os::unix::fs::PermissionsExt::from_mode(0o755));

    let _ = std::fs::remove_dir_all(&tmp);

    println!();
    println!("hboot updated successfully!");
}
