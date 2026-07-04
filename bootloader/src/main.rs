#![no_std]
#![no_main]

extern crate alloc;

use alloc::string::ToString;
use alloc::vec::Vec;
use uefi::boot;
use uefi::cstr16;
use uefi::fs::FileSystem;
use uefi::prelude::*;
use uefi::println;
use uefi::proto::console::text::{Input, Key};
use uefi::proto::media::file::{File, FileAttribute, FileMode};
use uefi::CString16;

mod config;
mod detect;
mod menu;
mod boot_loader;
mod util;

#[entry]
fn main() -> Status {
    uefi::helpers::init().unwrap();
    log::set_max_level(log::LevelFilter::Off);

    let mut cfg = load_config();
    let recovery_timeout = cfg.as_ref().map_or(5, |c| c.recovery_timeout);
    if check_recovery_key(recovery_timeout) {
        cfg = load_config();
    }
    let cfg = cfg.unwrap_or_else(|| config::Config {
        default: None,
        timeout: 5,
        recovery_timeout: 5,
        no_scan: false,
        order: None,
        entries: alloc::vec::Vec::new(),
    });

    let detected = if !cfg.no_scan { detect::scan_esp() } else { Vec::new() };

    let mut menu = menu::Menu::new(&cfg, detected);

    loop {
        match menu.run() {
            menu::MenuResult::Boot(entry) => {
                // Decrement boot counter before booting
                decrement_boot_counter(&entry);
                backup_entries();
                boot_loader::boot_entry(&entry);
                // Only reached on failure
                println!();
                println!("Boot failed. Press any key for options...");
                boot_loader::wait_for_key();
                recovery_menu();
            }
                menu::MenuResult::Manual => manual_boot(),
        }
    }
}

/// Decrement the boot counter for an entry by renaming the file.
/// e.g., arch+3.conf → arch+2.conf, arch+1.conf → arch.conf
fn decrement_boot_counter(entry: &config::Entry) {
    let source = match entry.source_path.as_ref() {
        Some(s) => s,
        None => return,
    };
    let counter = match entry.boot_counter {
        Some(c) if c > 0 => c,
        _ => return,
    };

    // source = \EFI\hboot\entries\arch+3.conf
    let dot_conf = source.len() - 5;
    let name_part = &source[..dot_conf];

    let plus = match name_part.rfind('+') {
        Some(p) => p,
        None => {
            println!("decrement_boot_counter: no '+' in filename '{}'", source);
            return;
        }
    };
    let base = &name_part[..plus];

    let new_name = if counter == 1 {
        alloc::format!("{}.conf", base)
    } else {
        alloc::format!("{}+{}.conf", base, counter - 1)
    };

    let fs = match get_fs() {
        Ok(f) => f,
        Err(e) => {
            println!("decrement_boot_counter: {}", e);
            return;
        }
    };
    let mut fs = fs;

    let normalized_old = util::normalize_path(source);
    let cstr_old = match CString16::try_from(normalized_old.as_str()) {
        Ok(c) => c,
        Err(_) => {
            println!("decrement_boot_counter: invalid path '{}'", source);
            return;
        }
    };
    let content = match fs.read(cstr_old.as_ref()) {
        Ok(c) => c,
        Err(_) => {
            println!("decrement_boot_counter: failed to read '{}'", source);
            return;
        }
    };

    let normalized_new = util::normalize_path(&new_name);
    let cstr_new = match CString16::try_from(normalized_new.as_str()) {
        Ok(c) => c,
        Err(_) => {
            println!("decrement_boot_counter: invalid new path '{}'", new_name);
            return;
        }
    };
    if fs.write(cstr_new.as_ref(), &content).is_err() {
        println!("decrement_boot_counter: failed to write '{}'", new_name);
        return;
    }

    // Delete old file using a separate protocol handle
    if let Ok(mut sfsp) = boot::get_image_file_system(boot::image_handle()) {
        if let Ok(mut volume) = sfsp.open_volume() {
            if let Ok(old_file) = volume.open(cstr_old.as_ref(), FileMode::ReadWrite, FileAttribute::empty()) {
                let _ = old_file.delete();
            }
        }
    }
}

fn get_stdin_system() -> Option<&'static mut Input> {
    let raw_st = uefi::table::system_table_raw()?;
    let st = unsafe { raw_st.as_ref() };
    if st.stdin.is_null() {
        return None;
    }
    Some(unsafe { &mut *(st.stdin.cast::<Input>()) })
}

fn manual_boot() {
    let mut fallback_guard: Option<boot::ScopedProtocol<Input>>;
    let input: &mut Input = if let Ok((guard, _)) = boot_loader::find_input() {
        fallback_guard = Some(guard);
        fallback_guard.as_mut().unwrap()
    } else if let Some(system_input) = get_stdin_system() {
        system_input
    } else {
        println!("No input device available.");
        boot_loader::wait_for_key();
        return;
    };
    let _ = input.reset(false);

    if let Some(entry) = menu::prompt_manual(input) {
        if entry.efi_path.is_empty() {
            return;
        }
        if !boot_loader::boot_entry(&entry) {
            println!("Boot failed. Press any key to continue...");
            boot_loader::wait_for_key();
        }
    }
}

fn recovery_menu() {
    let mut guard: Option<boot::ScopedProtocol<Input>>;
    let input: &mut Input = if let Ok((g, _)) = boot_loader::find_input() {
        guard = Some(g);
        guard.as_mut().unwrap()
    } else if let Some(s) = get_stdin_system() {
        s
    } else {
        boot_loader::reset_system()
    };

    let _ = input.reset(false);

    loop {
        println!();
        println!("===============================");
        println!("Recovery menu - what now?");
        println!("  m  Manual boot (type an .efi path)");
        println!("  b  Restore backup entries and retry");
        println!("  r  Reboot");
        println!("  f  Firmware setup");
        println!("  s  Shutdown");
        println!("-------------------------------");

        let key = loop {
            if let Ok(Some(k)) = input.read_key() {
                break k;
            }
            boot::stall(core::time::Duration::from_millis(10));
        };

        if let uefi::proto::console::text::Key::Printable(c) = key {
            let c_val: u16 = c.into();
            if c_val == b'm' as u16 || c_val == b'M' as u16 {
                manual_boot();
                continue;
            } else if c_val == b'b' as u16 || c_val == b'B' as u16 {
                println!("Restoring backup...");
                if restore_entries() {
                    println!("Backup restored. Press any key to reboot...");
                } else {
                    println!("No backup found. Press any key to continue...");
                }
                boot_loader::wait_for_key();
                boot_loader::reset_system();
            } else if c_val == b'r' as u16 || c_val == b'R' as u16 {
                boot_loader::reset_system();
            } else if c_val == b'f' as u16 || c_val == b'F' as u16 {
                unsafe {
                    let _ = boot::exit(
                        boot::image_handle(),
                        uefi::Status::SUCCESS,
                        0,
                        core::ptr::null_mut(),
                    );
                }
            } else if c_val == b's' as u16 || c_val == b'S' as u16 {
                uefi::runtime::reset(
                    uefi::runtime::ResetType::SHUTDOWN,
                    uefi::Status::SUCCESS,
                    None,
                );
            }
        }
    }
}

fn check_recovery_key(timeout_secs: u64) -> bool {
    if timeout_secs == 0 {
        return false;
    }
    let input = match get_stdin_system() {
        Some(i) => i,
        None => return false,
    };
    let _ = input.reset(false);

    let iterations = timeout_secs * 100;
    println!("Press r for recovery ({}s)...", timeout_secs);

    for _ in 0..iterations {
        if let Ok(Some(key)) = input.read_key() {
            if let Key::Printable(c) = key {
                let c_val: u16 = c.into();
                if c_val == b'r' as u16 || c_val == b'R' as u16 {
                    println!();
                    println!("Recovery key pressed. Restoring backup...");
                    if restore_entries() {
                        println!("Backup restored from /EFI/hboot/backup/entries/");
                    } else {
                        println!("No backup found at /EFI/hboot/backup/entries/");
                    }
                    return true;
                }
            }
        }
        boot::stall(core::time::Duration::from_millis(10));
    }
    false
}

fn ensure_backup_dir() {
    if let Ok(mut sfsp) = boot::get_image_file_system(boot::image_handle()) {
        if let Ok(mut volume) = sfsp.open_volume() {
            for p in [cstr16!("\\EFI\\hboot\\backup"), cstr16!("\\EFI\\hboot\\backup\\entries")] {
                let _ = volume.open(p, FileMode::CreateReadWrite, FileAttribute::DIRECTORY);
            }
        }
    }
}

fn backup_entries() {
    let fs = match get_fs() {
        Ok(f) => f,
        Err(_) => return,
    };
    let mut fs = fs;

    let entries_dir = cstr16!("\\EFI\\hboot\\entries");
    if !fs.try_exists(entries_dir).unwrap_or(false) {
        return;
    }

    ensure_backup_dir();

    if let Ok(dir_iter) = fs.read_dir(entries_dir) {
        for entry_result in dir_iter {
            if let Ok(entry) = entry_result {
                let name = entry.file_name();
                let name_str = name.to_string();
                if name_str.ends_with(".conf") || name_str.ends_with(".CONF") {
                    let src_path = alloc::format!("\\EFI\\hboot\\entries\\{}", name_str);
                    let dst_path = alloc::format!("\\EFI\\hboot\\backup\\entries\\{}", name_str);

                    if let Ok(cstr_src) = CString16::try_from(src_path.as_str()) {
                        if let Ok(data) = fs.read(cstr_src.as_ref()) {
                            if let Ok(cstr_dst) = CString16::try_from(dst_path.as_str()) {
                                let _ = fs.write(cstr_dst.as_ref(), &data);
                            }
                        }
                    }
                }
            }
        }
    }
}

fn restore_entries() -> bool {
    let fs = match get_fs() {
        Ok(f) => f,
        Err(_) => return false,
    };
    let mut fs = fs;

    let backup_dir = cstr16!("\\EFI\\hboot\\backup\\entries");
    if !fs.try_exists(backup_dir).unwrap_or(false) {
        return false;
    }

    let mut restored = false;
    if let Ok(dir_iter) = fs.read_dir(backup_dir) {
        for entry_result in dir_iter {
            if let Ok(entry) = entry_result {
                let name = entry.file_name();
                let name_str = name.to_string();
                if name_str.ends_with(".conf") || name_str.ends_with(".CONF") {
                    let src_path = alloc::format!("\\EFI\\hboot\\backup\\entries\\{}", name_str);
                    let dst_path = alloc::format!("\\EFI\\hboot\\entries\\{}", name_str);

                    if let Ok(cstr_src) = CString16::try_from(src_path.as_str()) {
                        if let Ok(data) = fs.read(cstr_src.as_ref()) {
                            if let Ok(cstr_dst) = CString16::try_from(dst_path.as_str()) {
                                let _ = fs.write(cstr_dst.as_ref(), &data);
                                restored = true;
                            }
                        }
                    }
                }
            }
        }
    }
    restored
}

fn load_config() -> Option<config::Config> {
    let main_paths = [cstr16!("\\EFI\\hboot\\hboot.conf"), cstr16!("\\hboot.conf")];

    let fs = get_fs();
    let mut fs = match fs {
        Ok(f) => f,
        Err(_) => return None,
    };

    // Start with defaults
    let mut cfg = config::Config {
        default: None,
        timeout: 5,
        recovery_timeout: 5,
        no_scan: false,
        order: None,
        entries: Vec::new(),
    };

    // Read main config files for global settings + inline entries (backwards compat)
    for path in &main_paths {
        if let Ok(text) = fs.read_to_string(*path) {
            if let Ok(parsed) = config::Config::parse(&text.into_bytes()) {
                if cfg.default.is_none() {
                    cfg.default = parsed.default;
                }
                cfg.timeout = parsed.timeout;
                cfg.recovery_timeout = parsed.recovery_timeout;
                if cfg.order.is_none() {
                    cfg.order = parsed.order;
                }
                if parsed.no_scan {
                    cfg.no_scan = true;
                }
                // Inline entries (old format with [name] sections)
                for e in parsed.entries {
                    if !cfg.entries.iter().any(|x| x.name == e.name) {
                        cfg.entries.push(e);
                    }
                }
            }
        }
    }

    // Read entry files from \EFI\hboot\entries\*.conf
    let entries_dir = cstr16!("\\EFI\\hboot\\entries");
    if fs.try_exists(entries_dir).unwrap_or(false) {
        if let Ok(dir_iter) = fs.read_dir(entries_dir) {
            for entry_result in dir_iter {
                if let Ok(entry) = entry_result {
                    let name = entry.file_name();
                    let name_str = name.to_string();
                    if name_str.ends_with(".conf") || name_str.ends_with(".CONF") {
                        load_one_entry(
                            &mut cfg,
                            &mut fs,
                            &name_str,
                            &alloc::format!("\\EFI\\hboot\\entries\\{}", name_str),
                        );
                    }
                }
            }
        }
    }

    // Read BLS type-1 entries from \loader\entries\*.conf
    let bls_dir = cstr16!("\\loader\\entries");
    if fs.try_exists(bls_dir).unwrap_or(false) {
        if let Ok(dir_iter) = fs.read_dir(bls_dir) {
            for entry_result in dir_iter {
                if let Ok(entry) = entry_result {
                    let name = entry.file_name();
                    let name_str = name.to_string();
                    if name_str.ends_with(".conf") || name_str.ends_with(".CONF") {
                        let full_path = alloc::format!("\\loader\\entries\\{}", name_str);
                        let normalized = util::normalize_path(&full_path);
                        let cstr = CString16::try_from(normalized.as_str()).ok();
                        if let Some(c) = cstr {
                            if let Ok(data) = fs.read(c.as_ref()) {
                                // Parse boot counter from filename: arch+3.conf
                                let raw_name = name_str
                                    .trim_end_matches(".conf")
                                    .trim_end_matches(".CONF");
                                let entry_name = raw_name.split('+').next().unwrap_or(raw_name);
                                if let Ok(parsed) =
                                    config::Config::parse_bls_entry(entry_name, &data)
                                {
                                    let mut parsed = parsed;
                                    parsed.boot_counter = parse_counter(raw_name);
                                    parsed.source_path = Some(full_path);
                                    cfg.entries.retain(|e| e.name != parsed.name);
                                    cfg.entries.push(parsed);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    Some(cfg)
}

/// Load one hboot entry file, parsing boot counter from filename (arch+3.conf → counter=3).
fn load_one_entry(
    cfg: &mut config::Config,
    fs: &mut FileSystem,
    name_str: &str,
    full_path: &str,
) {
    let raw_name = name_str
        .trim_end_matches(".conf")
        .trim_end_matches(".CONF");
    let entry_name = raw_name.split('+').next().unwrap_or(raw_name);
    if let Ok(data) = read_entry_file(fs, full_path) {
        if let Ok(mut parsed) = config::Config::parse_entry_file(entry_name, &data) {
            parsed.boot_counter = parse_counter(raw_name);
            parsed.source_path = Some(full_path.to_string());
            cfg.entries.retain(|e| e.name != parsed.name);
            cfg.entries.push(parsed);
        }
    }
}

/// Parse boot counter from a filename like "arch+3" → Some(3), "arch" → None.
fn parse_counter(raw_name: &str) -> Option<u32> {
    if let Some(plus) = raw_name.rfind('+') {
        let suffix = &raw_name[plus + 1..];
        if !suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_digit()) {
            return suffix.parse().ok();
        }
    }
    None
}

fn read_entry_file(fs: &mut FileSystem, path: &str) -> Result<Vec<u8>, ()> {
    let normalized = util::normalize_path(path);
    let cstr = CString16::try_from(normalized.as_str()).map_err(|_| ())?;
    fs.read(cstr.as_ref()).map_err(|_| ())
}

fn get_fs() -> Result<uefi::fs::FileSystem, &'static str> {
    let sfsp = boot::get_image_file_system(boot::image_handle())
        .map_err(|_| "failed to open filesystem")?;
    Ok(uefi::fs::FileSystem::new(sfsp))
}
