use std::process::Command;

const VERSION: &str = env!("CARGO_PKG_VERSION");
const RELEASE_URL: &str = "https://github.com/person134/nexec/releases/latest/download";

pub fn update() {
    println!("nexec update v{}", VERSION);
    println!("Fetching latest release...");

    let uid = Command::new("id").arg("-u").output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    if uid != "0" {
        eprintln!("error: update requires root (need to write to /usr/bin and ESP)");
        std::process::exit(1);
    }

    let tmp = std::env::temp_dir().join("nexec_update");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap_or_else(|e| {
        eprintln!("error: failed to create temp dir: {}", e);
        std::process::exit(1);
    });

    let cli_path = tmp.join("nexec");
    let efi_path = tmp.join("nexec-efi.efi");

    println!("  Downloading nexec...");
    let status = Command::new("curl")
        .args(["-fsSL", "-o"])
        .arg(&cli_path)
        .arg(&format!("{}/nexec", RELEASE_URL))
        .status()
        .unwrap_or_else(|e| {
            eprintln!("error: failed to run curl: {}", e);
            eprintln!("  Is curl installed?");
            std::process::exit(1);
        });
    if !status.success() {
        eprintln!("error: failed to download nexec from releases");
        std::process::exit(1);
    }

    let _ = std::fs::set_permissions(&cli_path, std::os::unix::fs::PermissionsExt::from_mode(0o755));

    println!("  Downloading nexec-efi.efi...");
    let status = Command::new("curl")
        .args(["-fsSL", "-o"])
        .arg(&efi_path)
        .arg(&format!("{}/nexec-efi.efi", RELEASE_URL))
        .status()
        .unwrap_or_else(|e| {
            eprintln!("error: failed to run curl: {}", e);
            eprintln!("  Is curl installed?");
            std::process::exit(1);
        });
    if !status.success() {
        eprintln!("error: failed to download nexec-efi.efi from releases");
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

    println!("  Updating /usr/bin/nexec...");
    std::fs::copy(&cli_path, "/usr/bin/nexec").unwrap_or_else(|e| {
        eprintln!("error: failed to copy nexec to /usr/bin: {}", e);
        std::process::exit(1);
    });
    let _ = std::fs::set_permissions("/usr/bin/nexec", std::os::unix::fs::PermissionsExt::from_mode(0o755));

    let _ = std::fs::remove_dir_all(&tmp);

    println!();
    println!("nexec updated successfully!");
}
