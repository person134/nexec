use alloc::string::String;
use alloc::string::ToString;
use alloc::vec::Vec;
use core::ops::{Deref, DerefMut};
use uefi::boot;
use uefi::cstr16;
use uefi::proto::console::text::{Input, Key, ScanCode};
use uefi::boot::{OpenProtocolAttributes, OpenProtocolParams, SearchType};
use uefi::Identify;

use crate::config::{Config, Entry};
use crate::util;

enum MenuInput<'a> {
    Owned(boot::ScopedProtocol<Input>),
    Borrowed(&'a mut Input),
}

impl Deref for MenuInput<'_> {
    type Target = Input;
    fn deref(&self) -> &Input {
        match self {
            MenuInput::Owned(g) => g,
            MenuInput::Borrowed(i) => i,
        }
    }
}

impl DerefMut for MenuInput<'_> {
    fn deref_mut(&mut self) -> &mut Input {
        match self {
            MenuInput::Owned(g) => g,
            MenuInput::Borrowed(i) => i,
        }
    }
}

pub enum MenuResult {
    Boot(Entry),
    Manual,
    Recovery,
}

enum KeyAction {
    Nothing,
    Boot,
    Manual,
    Recovery,
}

pub struct Menu {
    pub entries: Vec<Entry>,
    pub selected: usize,
    pub timeout: u64,
}

impl Menu {
    pub fn new(cfg: &Config, detected: Vec<Entry>) -> Self {
        let mut entries: Vec<Entry> = detected;
        for e in &cfg.entries {
            // Skip entries with exhausted boot counter
            if e.boot_counter == Some(0) {
                continue;
            }
            let is_dup = entries.iter().any(|ee| ee.efi_path == e.efi_path);
            if !is_dup {
                entries.push(e.clone());
            }
        }

        if let Some(order) = &cfg.order {
            let mut ordered: Vec<Entry> = Vec::new();
            let mut remaining: Vec<Entry> = Vec::new();
            for name in order {
                let pos = entries.iter().position(|e| &e.name == name);
                if let Some(i) = pos {
                    ordered.push(entries.remove(i));
                }
            }
            remaining.append(&mut entries);
            ordered.append(&mut remaining);
            entries = ordered;
        }

        let selected = if let Some(ref def) = cfg.default {
            entries
                .iter()
                .position(|e| Some(&e.name) == Some(def))
                .unwrap_or(0)
        } else {
            0
        };
        Menu {
            entries,
            selected,
            timeout: cfg.timeout,
        }
    }

    fn input_from_system_table() -> Option<&'static mut Input> {
        let raw_st = uefi::table::system_table_raw()?;
        let st = unsafe { raw_st.as_ref() };
        if st.stdin.is_null() {
            return None;
        }
        Some(unsafe { &mut *(st.stdin.cast::<Input>()) })
    }

    fn open_any_input() -> Option<MenuInput<'static>> {
        if let Some(input) = Self::input_from_system_table() {
            let _ = input.reset(false);
            return Some(MenuInput::Borrowed(input));
        }

        if let Ok(handles) = boot::locate_handle_buffer(SearchType::ByProtocol(&Input::GUID)) {
            for handle in handles.iter() {
                if let Ok(mut g) = boot::open_protocol_exclusive::<Input>(*handle) {
                    let _ = g.reset(false);
                    return Some(MenuInput::Owned(g));
                }
                if let Ok(mut g) = unsafe {
                    boot::open_protocol::<Input>(
                        OpenProtocolParams {
                            handle: *handle,
                            agent: boot::image_handle(),
                            controller: None,
                        },
                        OpenProtocolAttributes::GetProtocol,
                    )
                } {
                    let _ = g.reset(false);
                    return Some(MenuInput::Owned(g));
                }
            }
        }

        None
    }

    pub fn run(&mut self) -> MenuResult {
        // timeout = 0: skip UI and boot the default/first entry immediately
        if self.timeout == 0 {
            return self.entries.get(self.selected).cloned()
                .map(MenuResult::Boot)
                .unwrap_or(MenuResult::Manual);
        }
        match Self::open_any_input() {
            Some(mut input) => self.run_with_input(&mut input),
            None => {
                self.entries.first().cloned().map(MenuResult::Boot).unwrap_or(MenuResult::Manual)
            }
        }
    }

    fn run_with_input(&mut self, input: &mut Input) -> MenuResult {
        let _ = input.reset(false);
        let mut remaining = self.timeout * 10;

        uefi::system::with_stdout(|g| {
            let _ = g.clear();
        });

        loop {
            if self.entries.is_empty() {
                draw_no_entries();
                match prompt_manual(input) {
                    Some(entry) => return MenuResult::Boot(entry),
                    None => continue,
                }
            }

            draw_menu(self, remaining);

            if remaining > 0 {
                if let Some(k) = poll_for_key(input, 100) {
                    remaining = 0;
                    match handle_key(k, self) {
                        KeyAction::Boot => {
                            return MenuResult::Boot(self.entries[self.selected].clone())
                        }
                        KeyAction::Manual => return MenuResult::Manual,
                        KeyAction::Recovery => return MenuResult::Recovery,
                        KeyAction::Nothing => {}
                    }
                }
            } else {
                if let Some(key) = read_key_blocking(input) {
                    match handle_key(key, self) {
                        KeyAction::Boot => {
                            return MenuResult::Boot(self.entries[self.selected].clone())
                        }
                        KeyAction::Manual => return MenuResult::Manual,
                        KeyAction::Recovery => return MenuResult::Recovery,
                        KeyAction::Nothing => {}
                    }
                }
            }

            if remaining > 0 {
                remaining = remaining.saturating_sub(1);
                if remaining == 0 && !self.entries.is_empty() {
                    return MenuResult::Boot(self.entries[self.selected].clone());
                }
            }
        }
    }
}

fn draw_no_entries() {
    uefi::system::with_stdout(|g| {
        g.clear().ok();
        let _ = g.set_cursor_position(0, 3);
        let _ = g.output_string(cstr16!(
            "No entries detected. Press m for manual boot, f for firmware setup.\r\n"
        ));
    });
}

fn draw_menu(menu: &Menu, remaining: u64) {
    uefi::system::with_stdout(|g| {
        let (cols, rows) = g
            .current_mode()
            .ok()
            .flatten()
            .map(|m| (m.columns(), m.rows()))
            .unwrap_or((80, 25));
        let menu_lines = 2 + menu.entries.len();
        let start_y = if menu_lines < rows { (rows - menu_lines) / 2 } else { 1 };

        let _ = g.set_cursor_position(0, start_y);

        let mut text = String::new();
        text.reserve(1024);

        let mut add_line = |s: &str| {
            let width = s.chars().count();
            let fill = cols.saturating_sub(1);
            let pad_x = if width < fill { (fill - width) / 2 } else { 0 };
            for _ in 0..pad_x {
                text.push(' ');
            }
            text.push_str(s);
            for _ in 0..fill.saturating_sub(pad_x + width) {
                text.push(' ');
            }
            text.push_str("\r\n");
        };

        for (i, entry) in menu.entries.iter().enumerate() {
            let marker = if i == menu.selected { " >" } else { "  " };
            let counter = entry.boot_counter.map_or(String::new(), |c| {
                if c > 0 { alloc::format!(" [{}]", c) } else { String::new() }
            });
            add_line(&alloc::format!("{}{}. {}{}", marker, i + 1, entry.title, counter));
        }
        add_line("-------------------------------");
        if remaining > 0 {
            add_line(&alloc::format!(
                "Boot default in {}s  \u{2191}\u{2193} Enter: boot  m: manual  r: recovery  f: firmware",
                remaining / 10
            ));
        } else {
            add_line(
                "\u{2191}\u{2193} Enter: boot  m: manual  r: recovery  f: firmware",
            );
        }

        let mut u16_buf = [0u16; 4096];
        if let Ok(cstr) = uefi::CStr16::from_str_with_buf(&text, &mut u16_buf) {
            let _ = g.output_string(cstr);
        }
    });
}

pub fn prompt_manual(input: &mut Input) -> Option<Entry> {
    uefi::system::with_stdout(|g| {
        let _ = g.set_cursor_position(0, 0);
        let _ = g.output_string(cstr16!(
            "Enter path to .efi file (e.g. /EFI/arch/systemd-bootx64.efi):\r\n"
        ));
        let _ = g.output_string(cstr16!(
            "Type the path and press Enter, or press Esc to go back.\r\n"
        ));
    });

    let mut buf: Vec<u8> = Vec::new();
    loop {
        let Some(key) = read_key_blocking(input) else { continue };
        match key {
            Key::Printable(c) => {
                let c_val: u16 = c.into();
                if c_val == b'\r' as u16 || c_val == b'\n' as u16 {
                    if !buf.is_empty() {
                        let path = core::str::from_utf8(&buf).unwrap_or("").to_string();
                        let normalized = util::normalize_path(&path);
                        return Some(Entry {
                            name: "manual".into(),
                            title: path,
                            efi_path: normalized,
                            options: None,
                            initrd: Vec::new(),
                            boot_counter: None,
                            source_path: None,
                        });
                    }
                } else if c_val == 8 || c_val == 127 {
                    buf.pop();
                } else if (32..=126).contains(&c_val) {
                    buf.push(c_val as u8);
                }
            }
            Key::Special(sc) if sc == ScanCode::ESCAPE => return None,
            _ => {}
        }
    }
}

fn poll_for_key(input: &mut Input, delay_ms: u64) -> Option<Key> {
    let iterations = delay_ms / 10;
    for _ in 0..iterations {
        // Try direct read first (works with most keyboards)
        if let Ok(Some(key)) = input.read_key() {
            return Some(key);
        }
        boot::stall(core::time::Duration::from_millis(10));
    }
    // Final attempt after loop
    input.read_key().ok().flatten()
}

fn read_key_blocking(input: &mut Input) -> Option<Key> {
    loop {
        match input.read_key() {
            Ok(Some(key)) => return Some(key),
            Ok(None) => {}
            Err(_) => {
                // Device error — treat as no key available to avoid hang
                return None;
            }
        }
        boot::stall(core::time::Duration::from_millis(10));
    }
}

fn handle_key(key: Key, menu: &mut Menu) -> KeyAction {
    match key {
        Key::Special(sc) if sc == ScanCode::UP => {
            if menu.selected > 0 {
                menu.selected -= 1;
            }
        }
        Key::Special(sc) if sc == ScanCode::DOWN => {
            if menu.selected < menu.entries.len().saturating_sub(1) {
                menu.selected += 1;
            }
        }
        Key::Special(sc) if sc == ScanCode::HOME => {
            menu.selected = 0;
        }
        Key::Special(sc) if sc == ScanCode::END => {
            menu.selected = menu.entries.len().saturating_sub(1);
        }
        Key::Printable(c) => {
            let c_val: u16 = c.into();
            if (c_val == b'\r' as u16 || c_val == b'\n' as u16) && !menu.entries.is_empty() {
                return KeyAction::Boot;
            }
            if c_val == b'f' as u16 || c_val == b'F' as u16 {
                boot_firmware();
            }
            if c_val == b'm' as u16 || c_val == b'M' as u16 {
                return KeyAction::Manual;
            }
            if c_val == b'r' as u16 || c_val == b'R' as u16 {
                return KeyAction::Recovery;
            }
            if c_val >= b'1' as u16 && c_val <= b'9' as u16 {
                let idx = (c_val - b'1' as u16) as usize;
                if idx < menu.entries.len() {
                    menu.selected = idx;
                    return KeyAction::Boot;
                }
            }
        }
        _ => {}
    }
    KeyAction::Nothing
}

fn boot_firmware() {
    unsafe {
        let _ = boot::exit(
            boot::image_handle(),
            uefi::Status::SUCCESS,
            0,
            core::ptr::null_mut(),
        );
    }
}
