use std::path::Path;

pub fn detect(esp_path: Option<String>) {
    let esp = esp_path.unwrap_or_else(|| {
        super::install::detect_esp().unwrap_or_else(|| {
            eprintln!("error: could not detect ESP. Specify with --esp");
            std::process::exit(1);
        })
    });
    let esp = esp.trim_end_matches('/');

    println!("ESP: {}", esp);
    println!();
    println!("Detected entries:");
    println!("-----------------");

    // Windows
    let windows_path = format!("{}/EFI/Microsoft/Boot/bootmgfw.efi", esp);
    if Path::new(&windows_path).exists() {
        println!("  Windows Boot Manager");
        println!("    efi: /EFI/Microsoft/Boot/bootmgfw.efi");
    }

    // UKIs
    let linux_dir = format!("{}/EFI/Linux", esp);
    if Path::new(&linux_dir).is_dir() {
        if let Ok(entries) = std::fs::read_dir(&linux_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if let Some(name) = path.file_name() {
                    let name = name.to_string_lossy();
                    if name.ends_with(".efi") || name.ends_with(".EFI") {
                        println!("  Linux UKI: {}", name);
                        println!("    efi: /EFI/Linux/{}", name);
                    }
                }
            }
        }
    }

    // Standalone kernels on ESP root (distribution-agnostic scan)
    if let Ok(dir) = std::fs::read_dir(esp) {
        let mut kernels: Vec<String> = dir
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|name| name.starts_with("vmlinuz"))
            .collect();
        kernels.sort();
        for name in kernels {
            let version = name.strip_prefix("vmlinuz-").unwrap_or("(no version)");
            println!("  Linux kernel: {}", name);
            println!("    version: {}", version);
            println!("    efi: /{}", name);
            for initrd_candidate in [
                format!("{}/initramfs-{}.img", esp, version),
                format!("{}/initrd.img-{}", esp, version),
            ] {
                if Path::new(&initrd_candidate).exists() {
                    let fname = initrd_candidate.rsplit('/').next().unwrap_or("");
                    println!("    initrd: /{}", fname);
                }
            }
        }
    }

    // nexec configs
    let config_paths = [
        format!("{}/EFI/nexec/nexec.conf", esp),
        format!("{}/EFI/nexec/entries", esp),
        format!("{}/nexec.conf", esp),
    ];
    for cp in &config_paths {
        if Path::new(cp).exists() {
            println!("  Config: {}", cp);
        }
    }
}
