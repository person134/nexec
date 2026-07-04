use alloc::string::String;
use alloc::string::ToString;
use alloc::vec;
use alloc::vec::Vec;
use uefi::boot;
use uefi::cstr16;
use uefi::fs::FileSystem;
use uefi::CString16;

use crate::config::Entry;
use crate::config::Config;
use crate::util;

pub fn scan_esp() -> Vec<Entry> {
    let mut fs = match get_fs() {
        Ok(f) => f,
        Err(_) => return Vec::new(),
    };

    let mut entries = Vec::new();

    // Windows
    if fs.try_exists(cstr16!("\\EFI\\Microsoft\\Boot\\bootmgfw.efi")).unwrap_or(false) {
        entries.push(Entry {
            name: "windows".into(),
            title: "Windows".into(),
            efi_path: "\\EFI\\Microsoft\\Boot\\bootmgfw.efi".into(),
            options: None,
            initrd: Vec::new(),
            boot_counter: None,
            source_path: None,
        });
    }

    // UKIs (kernel + initrd combined EFI images)
    if let Ok(ukis) = scan_ukis(&mut fs) {
        entries.extend(ukis);
    }

    // BLS type-1 entries (loader/entries/*.conf)
    if let Ok(bls) = scan_bls_entries(&mut fs) {
        entries.extend(bls);
    }

    // Standalone kernels on ESP root
    if let Ok(kernels) = scan_kernels(&mut fs) {
        entries.extend(kernels);
    }

    entries
}

fn get_fs() -> Result<FileSystem, &'static str> {
    let sfsp = boot::get_image_file_system(boot::image_handle())
        .map_err(|_| "failed to open filesystem")?;
    Ok(FileSystem::new(sfsp))
}

fn scan_ukis(fs: &mut FileSystem) -> Result<Vec<Entry>, &'static str> {
    let mut entries = Vec::new();
    if !fs.try_exists(cstr16!("\\EFI\\Linux")).unwrap_or(false) {
        return Ok(entries);
    }
    let dir_iter = fs.read_dir(cstr16!("\\EFI\\Linux")).map_err(|_| "cannot read dir")?;
    for entry in dir_iter {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let name = entry.file_name();
        let name_str = name.to_string();
        if name_str.ends_with(".efi") || name_str.ends_with(".EFI") {
            let path = alloc::format!("\\EFI\\Linux\\{}", name_str);
            let display = name_str.trim_end_matches(".efi").trim_end_matches(".EFI").replace('-', " ");
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
            let entry_name = name_str.to_lowercase().replace(".efi", "");
            entries.push(Entry {
                name: entry_name,
                title,
                efi_path: path,
                options: None,
                initrd: Vec::new(),
                boot_counter: None,
                source_path: None,
            });
        }
    }
    Ok(entries)
}

fn scan_kernels(fs: &mut FileSystem) -> Result<Vec<Entry>, &'static str> {
    let mut entries = Vec::new();

    let root_iter = fs.read_dir(cstr16!("\\")).map_err(|_| "cannot read root")?;
    for entry in root_iter {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let name = entry.file_name();
        let name_str = name.to_string();

        let version = if name_str == "vmlinuz" {
            Some("")
        } else if let Some(ver) = name_str.strip_prefix("vmlinuz-") {
            Some(ver)
        } else {
            None
        };

        if let Some(ver) = version {
            let efi_path = alloc::format!("\\{}", name_str);
            let entry_name = if ver.is_empty() {
                "linux".into()
            } else {
                alloc::format!("linux-{}", ver.to_lowercase())
            };
            let title = if ver.is_empty() {
                "Linux".into()
            } else {
                alloc::format!("Linux {}", ver)
            };

            let mut initrd = find_initrd(fs, ver);
            // Add microcode before kernel initramfs
            if let Some(ucode) = find_microcode(fs) {
                initrd.insert(0, ucode);
            }

            entries.push(Entry {
                name: entry_name,
                title,
                efi_path,
                options: None,
                initrd,
                boot_counter: None,
                source_path: None,
            });
        }
    }

    Ok(entries)
}

fn find_microcode(fs: &mut FileSystem) -> Option<String> {
    let candidates = ["\\intel-ucode.img", "\\amd-ucode.img"];
    for &candidate in &candidates {
        let cstr = CString16::try_from(candidate).ok()?;
        if fs.try_exists(&*cstr).unwrap_or(false) {
            return Some(candidate.into());
        }
    }
    None
}

/// Scan BLS type-1 entries from /loader/entries/*.conf
fn scan_bls_entries(fs: &mut FileSystem) -> Result<Vec<Entry>, &'static str> {
    let mut entries = Vec::new();
    if !fs.try_exists(cstr16!("\\loader\\entries")).unwrap_or(false) {
        return Ok(entries);
    }
    let dir_iter = fs.read_dir(cstr16!("\\loader\\entries")).map_err(|_| "cannot read loader/entries")?;
    for entry_result in dir_iter {
        let entry = match entry_result {
            Ok(e) => e,
            Err(_) => continue,
        };
        let name = entry.file_name();
        let name_str = name.to_string();
        if !name_str.ends_with(".conf") && !name_str.ends_with(".CONF") {
            continue;
        }
        let raw_name = name_str.trim_end_matches(".conf").trim_end_matches(".CONF");
        let entry_name = raw_name.split('+').next().unwrap_or(raw_name);
        let full_path = alloc::format!("\\loader\\entries\\{}", name_str);
        let normalized = util::normalize_path(&full_path);
        let cstr = CString16::try_from(normalized.as_str()).map_err(|_| "bad bls path")?;
        if let Ok(data) = fs.read(cstr.as_ref()) {
            if let Ok(parsed) = Config::parse_bls_entry(entry_name, &data) {
                entries.push(parsed);
            }
        }
    }
    Ok(entries)
}

fn find_initrd(fs: &mut FileSystem, version: &str) -> Vec<String> {
    let candidates: Vec<alloc::string::String> = if version.is_empty() {
        Vec::from([
            alloc::string::String::from("\\initramfs-linux.img"),
            alloc::string::String::from("\\initrd.img"),
            alloc::string::String::from("\\initramfs.img"),
        ])
    } else {
        Vec::from([
            alloc::format!("\\initramfs-{}.img", version),
            alloc::format!("\\initrd.img-{}", version),
            alloc::format!("\\initramfs-{}-generic.img", version),
        ])
    };

    for p in &candidates {
        let normalized = util::normalize_path(p);
        if let Ok(cstr) = CString16::try_from(normalized.as_str()) {
            if fs.try_exists(&*cstr).unwrap_or(false) {
                return vec![p.clone()];
            }
        }
    }
    Vec::new()
}
