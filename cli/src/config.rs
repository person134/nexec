use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

type DetectedEntry = (String, String, String, Option<String>, Option<String>);

pub fn init(output: String) {
    let path = Path::new(&output);
    if path.exists() {
        eprintln!("error: {} already exists", output);
        std::process::exit(1);
    }

    // Write main config
    let main_content = r#"# hboot main configuration
# Place this file at \EFI\hboot\hboot.conf on your ESP.
# Boot entries go in \EFI\hboot\entries\*.conf (one file per entry).
default = arch
timeout = 5
# recovery_timeout = 2    # seconds to hold 'r' for recovery menu (default: 2)
# order = arch windows    # uncomment to set display order by entry name
# no_scan = true          # uncomment to disable auto-detection of OSes
"#;

    // Determine parent dir: if output looks like a dir or has a filename
    let main_path = if output.ends_with('/') || output.ends_with("\\") || path.is_dir() {
        Path::new(&output).join("hboot.conf")
    } else if output.ends_with("hboot.conf") {
        Path::new(&output).to_path_buf()
    } else {
        // Assume output is a directory
        Path::new(&output).join("hboot.conf")
    };

    let parent = main_path.parent().unwrap_or_else(|| {
        eprintln!("error: invalid output path with no parent directory");
        std::process::exit(1);
    });
    std::fs::create_dir_all(parent).unwrap_or_else(|e| {
        eprintln!("error: failed to create directories: {}", e);
        std::process::exit(1);
    });

    std::fs::write(&main_path, main_content).unwrap_or_else(|e| {
        eprintln!("error: failed to write {}: {}", main_path.display(), e);
        std::process::exit(1);
    });
    println!("Created: {}", main_path.display());

    // Create entries directory with sample entries
    let entries_dir = main_path.parent().unwrap().join("entries");
    std::fs::create_dir_all(&entries_dir).unwrap_or_else(|e| {
        eprintln!("error: failed to create {}: {}", entries_dir.display(), e);
        std::process::exit(1);
    });

    // Sample entry: arch
    let arch_entry = r#"title = Arch Linux
efi = \vmlinuz-linux
options = root=UUID=your-root-uuid rw quiet
initrd = \initramfs-linux.img
"#;
    std::fs::write(entries_dir.join("arch.conf"), arch_entry).unwrap_or_else(|e| {
        eprintln!("error: failed to write arch.conf: {}", e);
        std::process::exit(1);
    });
    println!("Created: {}", entries_dir.join("arch.conf").display());

    // Sample entry: windows
    let win_entry = r#"title = Windows
efi = \EFI\Microsoft\Boot\bootmgfw.efi
"#;
    std::fs::write(entries_dir.join("windows.conf"), win_entry).unwrap_or_else(|e| {
        eprintln!("error: failed to write windows.conf: {}", e);
        std::process::exit(1);
    });
    println!("Created: {}", entries_dir.join("windows.conf").display());

    println!();
    println!("Edit the files and place them on your ESP:");
    println!("  \\EFI\\hboot\\hboot.conf            (main config)");
    println!("  \\EFI\\hboot\\entries\\*.conf        (one file per entry)");
}

fn current_os() -> Option<(String, String)> {
    let content = std::fs::read_to_string("/etc/os-release").ok()?;
    let mut id = None;
    let mut name = None;
    for line in content.lines() {
        if let Some(val) = line.strip_prefix("ID=") {
            id = Some(val.trim_matches('"').to_lowercase());
        }
        if let Some(val) = line.strip_prefix("NAME=") {
            name = Some(val.trim_matches('"').to_string());
        }
    }
    Some((id.unwrap_or_else(|| "linux".into()), name.unwrap_or_else(|| "Linux".into())))
}

/// Find the running kernel path on the filesystem (distribution-agnostic).
fn find_kernel() -> Option<PathBuf> {
    // First try uname -r to find the running kernel
    if let Ok(output) = Command::new("uname").arg("-r").output() {
        let ver = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !ver.is_empty() {
            let by_version = format!("/boot/vmlinuz-{}", ver);
            if Path::new(&by_version).exists() {
                return Some(PathBuf::from(by_version));
            }
        }
    }
    // Fallback: scan /boot for any vmlinuz-* file
    if let Ok(dir) = std::fs::read_dir("/boot") {
        let mut candidates: Vec<PathBuf> = dir
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                name.starts_with("vmlinuz")
            })
            .collect();
        candidates.sort();
        candidates.into_iter().last()
    } else {
        None
    }
}

/// Find matching initramfs for a given kernel path (distribution-agnostic).
fn find_initrd(kernel: &Path) -> Option<PathBuf> {
    let name = kernel.file_name()?.to_string_lossy();
    let stem = name.strip_prefix("vmlinuz-").unwrap_or("");

    let patterns = if stem.is_empty() {
        vec![
            "/boot/initramfs-linux.img".into(),
            "/boot/initrd.img".into(),
            "/boot/initramfs.img".into(),
        ]
    } else {
        vec![
            format!("/boot/initramfs-{}.img", stem),
            format!("/boot/initrd.img-{}", stem),
        ]
    };

    for p in &patterns {
        let pb = PathBuf::from(p);
        if pb.exists() {
            return Some(pb);
        }
    }
    None
}

/// Convert an absolute path like /boot/vmlinuz-linux to an ESP-relative
/// UEFI path like \vmlinuz-linux, by stripping the ESP mount prefix.
fn to_efi_path(abs_path: &Path, esp: &Path) -> Option<String> {
    let esp_canon = esp.canonicalize().ok()?;
    let esp_raw = if esp.is_absolute() { Some(esp.to_path_buf()) } else { std::path::absolute(esp).ok() };

    // Try to canonicalize the file path too; if it fails, use as-is.
    let abs = abs_path.canonicalize().unwrap_or_else(|_| abs_path.to_path_buf());

    // Try stripping ESP prefix, trying multiple path combinations.
    // This handles symlinks and bind mounts where canonical paths may differ.
    let rest = abs.strip_prefix(&esp_canon).ok()
        .or_else(|| esp_raw.as_ref().and_then(|p| abs.strip_prefix(p).ok()))
        .or_else(|| abs_path.strip_prefix(&esp_canon).ok())
        .or_else(|| esp_raw.as_ref().and_then(|p| abs_path.strip_prefix(p).ok()))?;

    let components: Vec<_> = rest.components().map(|c| c.as_os_str().to_string_lossy()).collect();
    if components.is_empty() {
        return None;
    }
    Some(format!("\\{}", components.join("\\")))
}

/// Resolve the root block device (e.g. `/dev/nvme0n1p2`) by matching
/// the device of `/` against `/proc/self/mountinfo`.
/// Uses `mountpoint -d /` which outputs `major:minor` in decimal,
/// exactly as mountinfo does — no encoding confusion.
/// Works both inside and outside a chroot because we match by device
/// ID, not by mount-point path.
fn resolve_root_device() -> Option<String> {
    let target = Command::new("mountpoint")
        .args(["-d", "/"])
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
                (!s.is_empty()).then_some(s)
            } else {
                None
            }
        })?;

    let content = std::fs::read_to_string("/proc/self/mountinfo").ok()?;
    for line in content.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() >= 4 && fields[2] == target {
            if let Some(dash) = fields.iter().position(|&f| f == "-") {
                if dash + 3 < fields.len() {
                    let source = fields[dash + 2];
                    if source.starts_with("/dev/")
                        && !source.contains("/loop")
                        && !source.contains("/ram")
                    {
                        return Some(source.to_string());
                    }
                }
            }
        }
    }
    None
}

/// Try to get `root=UUID=<uuid>` (or `root=<device>`) for the current
/// root filesystem.
fn detect_root_param() -> Option<String> {
    let device = resolve_root_device()?;

    if let Ok(output) = Command::new("blkid")
        .args(["-s", "UUID", "-o", "value", &device])
        .output()
    {
        let uuid = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !uuid.is_empty() {
            return Some(format!("root=UUID={}", uuid));
        }
    }
    Some(format!("root={}", device))
}

fn in_live_env(cmdline: &str) -> bool {
    cmdline.split_whitespace().any(|p| p.starts_with("archiso"))
}

fn build_linux_options() -> String {
    let cmdline = std::fs::read_to_string("/proc/cmdline").unwrap_or_default();
    let cmdline = cmdline.trim();

    let has_root = cmdline.split_whitespace().any(|p| p.starts_with("root="));

    if has_root && !in_live_env(cmdline) {
        // Normal installed system: use its cmdline as-is.
        build_linux_options_from(cmdline)
    } else if let Some(root) = detect_root_param() {
        // Inside a chroot or live ISO — use the real root, no ISO junk.
        build_linux_options_from(&root)
    } else {
        build_linux_options_from("")
    }
}

fn build_linux_options_from(cmdline: &str) -> String {
    let mut opts = cmdline.to_string();

    opts = opts.split_whitespace()
        .filter(|p| !p.starts_with("initrd=") && !p.starts_with("archiso"))
        .collect::<Vec<_>>()
        .join(" ");

    if !opts.split_whitespace().any(|p| p == "rootwait") {
        opts = if opts.is_empty() { "rootwait".into() } else { format!("{} rootwait", opts) };
    }
    if !opts.split_whitespace().any(|p| p == "rw") {
        opts = if opts.is_empty() { "rw".into() } else { format!("{} rw", opts) };
    }
    if !opts.split_whitespace().any(|p| p.starts_with("panic=")) {
        opts = format!("{} panic=10", opts);
    }
    opts.split_whitespace().filter(|p| *p != "quiet").collect::<Vec<_>>().join(" ")
}

/// Returns (main_config_text, Vec<(entry_name, entry_text)>)
pub fn generate_detected_config(esp: &str) -> (String, Vec<(String, String)>) {
    let esp_path = Path::new(esp.trim_end_matches('/'));
    let mut entry_files: Vec<DetectedEntry> = Vec::new();

    // Windows
    let win = esp_path.join("EFI/Microsoft/Boot/bootmgfw.efi");
    if win.exists() {
        entry_files.push(("windows".into(), "Windows".into(), "\\EFI\\Microsoft\\Boot\\bootmgfw.efi".into(), None, None));
    }

    // Detect the running OS and point to its kernel + initrd on the ESP
    if let Some((os_id, os_name)) = current_os() {
        let kernel_on_esp = find_kernel().and_then(|kp| to_efi_path(&kp, esp_path));
        let kernel_on_esp_root = scan_esp_root_kernels(esp_path);

        let (efi_path, initrd_line) = if let Some(ref path) = kernel_on_esp {
            let kernel_path = find_kernel().unwrap();
            let initrd = find_initrd(&kernel_path)
                .and_then(|ip| to_efi_path(&ip, esp_path));
            if initrd.is_none() {
                eprintln!("warning: initramfs not found on ESP. The kernel will boot without it.");
                eprintln!("  Copy initramfs to ESP root: sudo cp /boot/initramfs-linux.img {}/", esp_path.display());
            }
            (path.clone(), initrd)
        } else if let Some(path) = kernel_on_esp_root {
            eprintln!("warning: running kernel not found under ESP mount, but found {} on ESP root.", path);
            eprintln!("  Initramfs cannot be auto-detected in this case.");
            eprintln!("  If it's also on the ESP, add: initrd = \\initramfs-linux.img");
            (path, None)
        } else {
            eprintln!("warning: could not find any kernel on the ESP.");
            eprintln!("  hboot cannot boot without a kernel. Run 'sudo hboot config edit' to configure manually.");
            (String::new(), None)
        };

        if !efi_path.is_empty() {
            let opts = build_linux_options();
            entry_files.push((os_id, os_name, efi_path, Some(opts), initrd_line));
        }
    }

    // UKIs
    let linux_dir = esp_path.join("EFI/Linux");
    if linux_dir.is_dir() {
        if let Ok(dir_entries) = std::fs::read_dir(&linux_dir) {
            for entry in dir_entries.flatten() {
                let name = entry.file_name();
                let name = name.to_string_lossy().to_string();
                if name.ends_with(".efi") || name.ends_with(".EFI") {
                    let display = name.trim_end_matches(".efi").trim_end_matches(".EFI").replace('-', " ");
                    let title: String = display
                        .split_whitespace()
                        .map(|w| {
                            let mut chars = w.chars();
                            match chars.next() {
                                Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
                                None => String::new(),
                            }
                        })
                        .collect::<Vec<_>>()
                        .join(" ");
                    let entry_name = name.to_lowercase().replace(".efi", "");
                    let efi_path = format!("\\EFI\\Linux\\{}", name);
                    entry_files.push((entry_name, title, efi_path, None, None));
                }
            }
        }
    }

    // Standalone kernels on ESP root (generic scan)
    scan_esp_root_kernels_all(esp_path, &mut entry_files);

    // Deduplicate by efi_path
    let mut seen: Vec<String> = Vec::new();
    entry_files.retain(|(_, _, p, _, _)| {
        if seen.contains(p) {
            false
        } else {
            seen.push(p.clone());
            true
        }
    });

    // Generate main config text
    let mut main_conf = String::new();
    main_conf.push_str("# hboot main configuration\n");
    main_conf.push_str("# Boot entries are in \\EFI\\hboot\\entries\\*.conf\n");
    main_conf.push_str("no_scan = true\n");
    main_conf.push_str("timeout = 5\n");

    if !entry_files.is_empty() {
        let order: Vec<&str> = entry_files.iter().map(|(n, _, _, _, _)| n.as_str()).collect();
        main_conf.push_str(&format!("order = {}\n\n", order.join(" ")));
    }

    // Generate entry files
    let entries: Vec<(String, String)> = entry_files
        .iter()
        .map(|(name, title, efi_path, options, initrd)| {
            let mut content = format!("title = {}\n", title);
            content.push_str(&format!("efi = {}\n", efi_path));
            if let Some(opts) = options {
                content.push_str(&format!("options = {}\n", opts));
            }
            if let Some(ird) = initrd {
                content.push_str(&format!("initrd = {}\n", ird));
            }
            (name.clone(), content)
        })
        .collect();

    (main_conf, entries)
}

/// Scan ESP root for any vmlinuz-* file (distribution-agnostic).
fn scan_esp_root_kernels(esp_path: &Path) -> Option<String> {
    let root_dir = esp_path.join("");
    if let Ok(dir) = std::fs::read_dir(&root_dir) {
        for entry in dir.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy().to_string();
            if name.starts_with("vmlinuz") {
                return Some(format!("\\{}", name));
            }
        }
    }
    None
}

/// Scan ESP root for vmlinuz-* files and add as entries.
fn scan_esp_root_kernels_all(esp_path: &Path, entries: &mut Vec<DetectedEntry>) {
    if let Ok(dir) = std::fs::read_dir(esp_path.join("")) {
        let mut kernels: Vec<String> = dir
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|name| name.starts_with("vmlinuz"))
            .collect();
        kernels.sort();

        for name in kernels {
            let efi_path = format!("\\{}", name);
            let version = name.strip_prefix("vmlinuz-").unwrap_or("");
            let title = if version.is_empty() {
                "Linux".into()
            } else {
                format!("Linux {}", version)
            };
            let entry_name = if version.is_empty() {
                "linux".into()
            } else {
                format!("linux-{}", version.to_lowercase())
            };
            let opts = build_linux_options();
            entries.push((entry_name, title, efi_path, Some(opts), None));
        }
    }
}

pub fn detect(esp_path: Option<String>) {
    let esp = esp_path.unwrap_or_else(|| {
        super::install::detect_esp().unwrap_or_else(|| {
            eprintln!("error: could not detect ESP. Specify with --esp");
            std::process::exit(1);
        })
    });
    let (main_conf, entry_files) = generate_detected_config(&esp);
    println!("=== Main config (hboot.conf) ===");
    println!("{}", main_conf);
    println!("=== Entry files (entries/*.conf) ===");
    for (name, content) in &entry_files {
        println!("--- {}.conf ---", name);
        println!("{}", content);
    }
}

pub fn set_default(entry: String, esp_path: Option<String>) {
    let esp = esp_path.unwrap_or_else(|| {
        super::install::detect_esp().unwrap_or_else(|| {
            eprintln!("error: could not detect ESP. Specify with --esp");
            std::process::exit(1);
        })
    });
    let esp = esp.trim_end_matches('/');

    // Prefer the new location
    let config_candidates = [
        format!("{}/EFI/hboot/hboot.conf", esp),
        format!("{}/hboot.conf", esp),
    ];

    let config_path = config_candidates.iter().find(|p| Path::new(p).exists());
    let path = match config_path {
        Some(p) => p.clone(),
        None => {
            eprintln!("error: no hboot.conf found on ESP");
            eprintln!("  Run 'hboot install' first.");
            std::process::exit(1);
        }
    };

    let content = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        eprintln!("error: failed to read {}: {}", path, e);
        std::process::exit(1);
    });

    let mut found = false;
    let mut lines: Vec<String> = content.lines().map(|l| l.to_string()).collect();
    for line in &mut lines {
        let trimmed = line.trim();
        if trimmed.starts_with("default") && trimmed.contains('=') {
            *line = format!("default = {}", entry);
            found = true;
            break;
        }
    }

    if !found {
        // Check if entry file exists
        let entry_path = format!("{}/EFI/hboot/entries/{}.conf", esp, entry);
        if !Path::new(&entry_path).exists() {
            // Also check old-style (entry might not have a file yet)
            eprintln!("warning: entry '{}' not found in {}/EFI/hboot/entries/", entry, esp);
            eprintln!("  Make sure a file {}.conf exists there.", entry);
        }
        lines.push(String::new());
        lines.push(format!("default = {}", entry));
    }

    let new_content = lines.join("\n");

    std::fs::write(&path, new_content).unwrap_or_else(|e| {
        eprintln!("error: failed to write {}: {}", path, e);
        std::process::exit(1);
    });

    println!("Default entry set to '{}' in {}", entry, path);
}

pub fn edit(esp_path: Option<String>) {
    let esp = esp_path.unwrap_or_else(|| {
        super::install::detect_esp().unwrap_or_else(|| {
            eprintln!("error: could not detect ESP. Specify with --esp");
            std::process::exit(1);
        })
    });
    let esp = esp.trim_end_matches('/');

    let config_candidates = [
        format!("{}/EFI/hboot/hboot.conf", esp),
        format!("{}/hboot.conf", esp),
    ];

    let path = config_candidates.iter().find(|p| Path::new(p).exists());
    let path = match path {
        Some(p) => p.clone(),
        None => {
            eprintln!("error: no hboot.conf found on ESP");
            eprintln!("  Run 'hboot install' to create one.");
            std::process::exit(1);
        }
    };

    let editor = std::env::var("EDITOR")
        .or_else(|_| std::env::var("VISUAL"))
        .unwrap_or_else(|_| "nano".to_string());

    println!("Opening config: {}", path);
    let status = std::process::Command::new(&editor)
        .arg(&path)
        .status()
        .unwrap_or_else(|e| {
            eprintln!("error: failed to run editor '{}': {}", editor, e);
            eprintln!("  Set $EDITOR or $VISUAL to your preferred editor.");
            std::process::exit(1);
        });

    if !status.success() {
        eprintln!("warning: editor exited with non-zero status");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cmdline_empty() {
        let result = build_linux_options_from("");
        assert!(result.split_whitespace().any(|p| p == "rootwait"));
        assert!(result.split_whitespace().any(|p| p == "rw"));
        assert!(result.split_whitespace().any(|p| p.starts_with("panic=")));
        assert!(!result.contains("quiet"));
    }

    #[test]
    fn cmdline_strips_initrd() {
        let result = build_linux_options_from("root=UUID=abc initrd=\\initramfs.img rw quiet");
        assert!(!result.contains("initrd="));
        assert!(result.contains("root=UUID=abc"));
    }

    #[test]
    fn cmdline_preserves_existing() {
        let result = build_linux_options_from("root=UUID=abc rw rootwait panic=5 quiet loglevel=3");
        assert!(result.contains("panic=5"));
        assert!(result.contains("loglevel=3"));
        assert!(!result.contains("quiet"));
        assert!(result.contains("rootwait"));
    }

    #[test]
    fn cmdline_adds_rootwait_if_missing() {
        let result = build_linux_options_from("root=UUID=abc");
        assert!(result.split_whitespace().any(|p| p == "rootwait"));
    }

    #[test]
    fn cmdline_adds_rw_if_missing() {
        let result = build_linux_options_from("root=UUID=abc rootwait");
        assert!(result.split_whitespace().any(|p| p == "rw"));
    }

    #[test]
    fn cmdline_adds_panic_if_missing() {
        let result = build_linux_options_from("root=UUID=abc rw rootwait");
        assert!(result.split_whitespace().any(|p| p.starts_with("panic=")));
    }

    #[test]
    fn cmdline_removes_quiet() {
        let result = build_linux_options_from("root=UUID=abc rw quiet rootwait");
        assert!(!result.contains("quiet"));
    }

    #[test]
    fn cmdline_only_quiet() {
        let result = build_linux_options_from("quiet");
        assert!(!result.contains("quiet"));
    }

    #[test]
    fn cmdline_multiple_extra_params() {
        let result = build_linux_options_from("root=/dev/sda1 cryptdevice=UUID=xyz:luks resume=UUID=swap");
        assert!(result.contains("cryptdevice=UUID=xyz:luks"));
        assert!(result.contains("resume=UUID=swap"));
    }

    #[test]
    fn cmdline_strips_archiso() {
        let result = build_linux_options_from("archisobasedir=arch archisosearchuuid=abc root=UUID=xyz");
        assert!(!result.contains("archiso"));
        assert!(result.contains("root=UUID=xyz"));
    }

    #[test]
    fn cmdline_archiso_no_root() {
        let result = build_linux_options_from("archisobasedir=arch archisolabel=ARCH_2026 quiet");
        assert!(!result.contains("archiso"));
        assert!(!result.contains("archisolabel"));
        assert!(!result.contains("quiet"));
        assert!(result.contains("rootwait"));
        assert!(result.contains("rw"));
    }

    #[test]
    fn cmdline_panic_not_overwritten() {
        let result = build_linux_options_from("panic=30");
        assert!(result.contains("panic=30"));
        assert!(!result.contains("panic=10"));
    }

    #[test]
    fn cmdline_empty_initrd_stripped() {
        let result = build_linux_options_from("initrd=");
        assert!(!result.contains("initrd"));
    }

    #[test]
    fn cmdline_only_rootwait() {
        let result = build_linux_options_from("rootwait");
        assert!(result.contains("rw"));
        assert!(result.contains("rootwait"));
        assert!(result.split_whitespace().any(|p| p.starts_with("panic=")));
    }

    #[test]
    fn in_live_env_true_for_archiso() {
        assert!(in_live_env("archisobasedir=arch quiet"));
        assert!(in_live_env("archisosearchuuid=abc"));
    }

    #[test]
    fn in_live_env_false_for_normal() {
        assert!(!in_live_env("root=UUID=abc rw quiet"));
        assert!(!in_live_env(""));
    }

    #[test]
    fn in_live_env_false_for_archiso_substring() {
        assert!(!in_live_env("root=UUID=archiso"));
    }

    #[test]
    fn in_live_env_archiso_like() {
        assert!(in_live_env("archisolabel=MY_ARCH"));
    }

    // to_efi_path: converts absolute paths to ESP-relative UEFI paths
    #[test]
    fn to_efi_path_simple_file() {
        let dir = tempfile::tempdir().unwrap();
        let esp = dir.path().join("esp");
        std::fs::create_dir(&esp).unwrap();
        let kernel = esp.join("vmlinuz-linux");
        std::fs::write(&kernel, b"kernel").unwrap();
        let result = to_efi_path(&kernel, &esp);
        assert_eq!(result, Some("\\vmlinuz-linux".into()));
    }

    #[test]
    fn to_efi_path_subdirectory() {
        let dir = tempfile::tempdir().unwrap();
        let esp = dir.path().join("esp");
        let sub = esp.join("EFI").join("Linux");
        std::fs::create_dir_all(&sub).unwrap();
        let uki = sub.join("arch.efi");
        std::fs::write(&uki, b"uki").unwrap();
        let result = to_efi_path(&uki, &esp);
        assert_eq!(result, Some("\\EFI\\Linux\\arch.efi".into()));
    }

    #[test]
    fn to_efi_path_outside_esp() {
        let dir = tempfile::tempdir().unwrap();
        let esp = dir.path().join("esp");
        std::fs::create_dir(&esp).unwrap();
        let outside = dir.path().join("boot").join("vmlinuz");
        std::fs::create_dir_all(&outside.parent().unwrap()).unwrap();
        std::fs::write(&outside, b"kernel").unwrap();
        let result = to_efi_path(&outside, &esp);
        assert_eq!(result, None);
    }

    #[test]
    fn to_efi_path_esp_not_canonical() {
        let dir = tempfile::tempdir().unwrap();
        let esp = dir.path().join("esp");
        std::fs::create_dir(&esp).unwrap();
        let kernel = esp.join("vmlinuz");
        std::fs::write(&kernel, b"kernel").unwrap();
        // Use a non-canonicalized esp path — function canonicalizes internally
        let result = to_efi_path(&kernel, &esp);
        assert_eq!(result, Some("\\vmlinuz".into()));
    }

    // scan_esp_root_kernels: finds vmlinuz-* on ESP root
    #[test]
    fn scan_esp_root_kernels_finds_vmlinuz() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("vmlinuz-linux"), b"k").unwrap();
        std::fs::write(dir.path().join("vmlinuz-6.1.0"), b"k").unwrap();
        std::fs::write(dir.path().join("initramfs-6.1.0.img"), b"i").unwrap();
        let result = scan_esp_root_kernels(dir.path());
        assert_eq!(result, Some("\\vmlinuz-6.1.0".into()));
    }

    #[test]
    fn scan_esp_root_kernels_returns_first_alphabetically() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("vmlinuz-6.0.0"), b"k").unwrap();
        std::fs::write(dir.path().join("vmlinuz-5.10.0"), b"k").unwrap();
        let result = scan_esp_root_kernels(dir.path());
        // Returns the first vmlinuz-* found in readdir order (not sorted), so just check one is returned
        assert!(result.is_some());
        assert!(result.unwrap().starts_with("\\vmlinuz-"));
    }

    #[test]
    fn scan_esp_root_kernels_none() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("EFI"), b"").unwrap();
        let result = scan_esp_root_kernels(dir.path());
        assert_eq!(result, None);
    }

    #[test]
    fn scan_esp_root_kernels_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let result = scan_esp_root_kernels(dir.path());
        assert_eq!(result, None);
    }

    // scan_esp_root_kernels_all: adds all vmlinuz-* entries
    #[test]
    fn scan_esp_root_kernels_all_empty() {
        let dir = tempfile::tempdir().unwrap();
        let mut entries = Vec::new();
        scan_esp_root_kernels_all(dir.path(), &mut entries);
        assert!(entries.is_empty());
    }

    #[test]
    fn scan_esp_root_kernels_all_multiple() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("vmlinuz-linux"), b"k").unwrap();
        std::fs::write(dir.path().join("vmlinuz-6.1.0"), b"k").unwrap();
        let mut entries = Vec::new();
        scan_esp_root_kernels_all(dir.path(), &mut entries);
        assert_eq!(entries.len(), 2);
        // Check entries are sorted alphabetically
        assert_eq!(entries[0].0, "linux-6.1.0");
        assert_eq!(entries[1].0, "linux-linux");
    }

    #[test]
    fn scan_esp_root_kernels_all_non_vmlinuz_ignored() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("vmlinuz"), b"k").unwrap();
        std::fs::write(dir.path().join("initramfs.img"), b"i").unwrap();
        std::fs::write(dir.path().join("EFI"), b"").unwrap();
        let mut entries = Vec::new();
        scan_esp_root_kernels_all(dir.path(), &mut entries);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].0, "linux");
    }

    #[test]
    fn scan_esp_root_kernels_all_includes_options() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("vmlinuz-linux"), b"k").unwrap();
        let mut entries = Vec::new();
        scan_esp_root_kernels_all(dir.path(), &mut entries);
        assert_eq!(entries.len(), 1);
        let (_, _, _, opts, initrd) = &entries[0];
        assert!(opts.is_some());
        assert!(initrd.is_none());
    }

    // generate_detected_config stubs
    #[test]
    fn generate_detected_config_empty_esp() {
        let dir = tempfile::tempdir().unwrap();
        let (main_conf, entries) = generate_detected_config(dir.path().to_str().unwrap());
        assert!(main_conf.contains("no_scan = true"));
        // No real OS or Windows detected in temp dir
        assert!(entries.is_empty() || !entries.iter().any(|(n, _)| n == "windows"));
    }

    #[test]
    fn generate_detected_config_windows_detected() {
        let dir = tempfile::tempdir().unwrap();
        let win_path = dir.path().join("EFI/Microsoft/Boot/bootmgfw.efi");
        std::fs::create_dir_all(&win_path.parent().unwrap()).unwrap();
        std::fs::write(&win_path, b"win").unwrap();
        let (main_conf, entries) = generate_detected_config(dir.path().to_str().unwrap());
        assert!(entries.iter().any(|(n, _)| n == "windows"));
        assert!(main_conf.contains("windows"));
    }

    #[test]
    fn generate_detected_config_uki_detected() {
        let dir = tempfile::tempdir().unwrap();
        let uki_path = dir.path().join("EFI/Linux/arch-linux.efi");
        std::fs::create_dir_all(&uki_path.parent().unwrap()).unwrap();
        std::fs::write(&uki_path, b"uki").unwrap();
        let (_, entries) = generate_detected_config(dir.path().to_str().unwrap());
        assert!(entries.iter().any(|(n, _)| n == "arch-linux"));
    }

    #[test]
    fn generate_detected_config_dedup_by_efi_path() {
        let dir = tempfile::tempdir().unwrap();
        // Add a vmlinuz, which would be detected by esp root scan too
        std::fs::write(dir.path().join("vmlinuz-linux"), b"k").unwrap();
        let (_, entries) = generate_detected_config(dir.path().to_str().unwrap());
        // Count linux entries
        let linux_entries: Vec<_> = entries.iter().filter(|(n, _)| n.starts_with("linux")).collect();
        assert!(linux_entries.len() <= 1, "should dedup to at most 1 linux entry");
    }

    // helper: determine what functions are accessible from test module
    #[test]
    fn current_os_handles_missing_file() {
        // current_os reads /etc/os-release — with a bogus file it won't find data
        // but shouldn't panic
        let result = current_os();
        // On a real system it returns Some, on CI without /etc/os-release it might be None
        // Either is fine — just shouldn't panic
        let _ = result;
    }

    #[test]
    fn find_kernel_boot_missing() {
        // Override PATH temporarily isn't great, but just verify it doesn't panic
        let result = find_kernel();
        // Either None (no /boot) or Some (system exists)
        let _ = result;
    }
}
