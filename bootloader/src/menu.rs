use alloc::format;
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec::Vec;
use core::ops::{Deref, DerefMut};
use uefi::boot;
use uefi::boot::{OpenProtocolAttributes, OpenProtocolParams, SearchType};
use uefi::proto::console::text::{Color, Input, Key, Output, ScanCode};
use uefi::cstr16;
use uefi::CStr16;
use uefi::Identify;

use crate::config::{Config, Entry};
use crate::util;

const VERSION: &str = env!("CARGO_PKG_VERSION");

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
    RestoreBackup,
}

enum KeyAction {
    Nothing,
    Boot,
    Manual,
    RestoreBackup,
}

pub struct Menu {
    pub entries: Vec<Entry>,
    pub selected: usize,
    pub timeout: u64,
}

// ---------------------------------------------------------------------------
// UEFI console helpers
// ---------------------------------------------------------------------------

fn write_str(g: &mut Output, s: &str) {
    let mut buf = [0u16; 2048];
    if let Ok(cstr) = CStr16::from_str_with_buf(s, &mut buf) {
        let _ = g.output_string(cstr);
    }
}

fn set_fg(g: &mut Output, color: Color) {
    let _ = g.set_color(color, Color::Black);
}

// ---------------------------------------------------------------------------
// Box-drawing primitives
// ---------------------------------------------------------------------------

fn box_width(cols: usize, title: &str, lines: &[String]) -> usize {
    let entry_max = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0);
    let f1 = "↑↓=boot  m=manual  b=backups";
    let f2 = "f=firmware  r=reboot";
    let max_content = title.chars().count().max(entry_max).max(f1.len()).max(f2.len());
    let desired = max_content + 6;
    ((cols as usize).saturating_sub(2)).min(52.max(desired))
}

fn draw_top(g: &mut Output, title: &str, w: usize) {
    set_fg(g, Color::Cyan);
    let dashes = "─".repeat(w.saturating_sub(title.len() + 4));
    write_str(g, &format!("┌ {} {}┐\r\n", title, dashes));
}

fn draw_bottom(g: &mut Output, w: usize) {
    set_fg(g, Color::Cyan);
    write_str(g, &format!("└{}┘\r\n", "─".repeat(w.saturating_sub(2))));
}

fn draw_empty(g: &mut Output, w: usize) {
    write_str(g, &format!("│{}│\r\n", " ".repeat(w.saturating_sub(2))));
}

fn draw_sep(g: &mut Output, w: usize) {
    set_fg(g, Color::DarkGray);
    write_str(g, &format!("│ {} │\r\n", "─".repeat(w.saturating_sub(4))));
}

fn draw_line(g: &mut Output, text: &str, w: usize, color: Color) {
    set_fg(g, color);
    let inner = w.saturating_sub(4);
    let tlen = text.chars().count();
    if tlen >= inner {
        let s: String = text.chars().take(inner).collect();
        write_str(g, &format!("│ {}│\r\n", s));
    } else {
        write_str(g, &format!("│ {}{} │\r\n", text, " ".repeat(inner - tlen)));
    }
}

fn draw_entry_line(g: &mut Output, entry: &Entry, idx: usize, selected: bool, w: usize) {
    let marker = if selected { ">" } else { " " };
    let num = format!("{}.", idx + 1);
    let counter = entry.boot_counter.map_or(String::new(), |c| {
        if c > 0 { format!(" [{}]", c) } else { String::new() }
    });

    let inner = w.saturating_sub(4);
    let prefix = format!("  {} {}", marker, num);
    let full = format!("{}{}{}", prefix, entry.title, counter);
    let flen = full.chars().count();

    write_str(g, "│ ");
    if selected {
        set_fg(g, Color::Green);
    } else {
        set_fg(g, Color::White);
    }
    write_str(g, &prefix);
    write_str(g, &entry.title);

    if !counter.is_empty() {
        set_fg(g, Color::Yellow);
        write_str(g, &counter);
    }

    let pad = inner.saturating_sub(flen);
    if pad > 0 {
        if selected {
            set_fg(g, Color::Green);
        } else {
            set_fg(g, Color::White);
        }
        write_str(g, &" ".repeat(pad));
    }
    set_fg(g, Color::Cyan);
    write_str(g, " │\r\n");
}

// ---------------------------------------------------------------------------
// Public: show a status/info dialog box
// ---------------------------------------------------------------------------

pub fn show_status(lines: &[(&str, Color)]) {
    uefi::system::with_stdout(|g| {
        let (cols, rows) = g.current_mode().ok().flatten()
            .map(|m| (m.columns(), m.rows()))
            .unwrap_or((80, 25));

        let title = format!("hboot v{}", VERSION);
        let max_line = lines.iter().map(|(t, _)| t.chars().count()).max().unwrap_or(0);
        let w = ((cols as usize).saturating_sub(2)).min(46.max(max_line + 8));
        let box_h = lines.len() + 4;
        let start_y = if box_h < rows { (rows - box_h) / 2 } else { 1 };

        g.clear().ok();
        g.set_cursor_position(0, start_y).ok();

        draw_top(g, &title, w);
        draw_empty(g, w);
        for (text, color) in lines {
            draw_line(g, text, w, *color);
        }
        draw_empty(g, w);
        draw_bottom(g, w);
    });
}

// ---------------------------------------------------------------------------
// Menu impl
// ---------------------------------------------------------------------------

impl Menu {
    pub fn new(cfg: &Config, detected: Vec<Entry>) -> Self {
        let mut entries: Vec<Entry> = detected;
        for e in &cfg.entries {
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
                        KeyAction::RestoreBackup => return MenuResult::RestoreBackup,
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
                        KeyAction::RestoreBackup => return MenuResult::RestoreBackup,
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

// ---------------------------------------------------------------------------
// Drawing functions
// ---------------------------------------------------------------------------

fn draw_no_entries() {
    uefi::system::with_stdout(|g| {
        g.clear().ok();
        let w = 46;
        let title = format!("hboot v{}", VERSION);
        draw_top(g, &title, w);
        draw_empty(g, w);
        draw_line(g, "No entries detected", w, Color::White);
        draw_empty(g, w);
        draw_line(g, "m  manual boot", w, Color::Cyan);
        draw_line(g, "f  firmware setup", w, Color::Cyan);
        draw_empty(g, w);
        draw_bottom(g, w);
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

        let title = format!("hboot v{}", VERSION);
        let foot1 = "↑↓=boot  m=manual  b=backups";
        let foot2 = "f=firmware  r=reboot";

        let mut lines: Vec<String> = Vec::new();
        for (i, e) in menu.entries.iter().enumerate() {
            let marker = if i == menu.selected { " >" } else { "  " };
            let counter = e.boot_counter.map_or(String::new(), |c| {
                if c > 0 { format!(" [{}]", c) } else { String::new() }
            });
            lines.push(format!("{}{}. {}{}", marker, i + 1, e.title, counter));
        }

        let w = box_width(cols, &title, &lines);
        let inner = w.saturating_sub(4);

        let countdown = if remaining > 0 {
            Some(format!("Auto-boot in {}s", remaining / 10))
        } else {
            None
        };
        let extra = countdown.as_ref().map_or(0, |_| 1);
        let box_h = menu.entries.len() + 8 + extra;
        let start_y = if box_h < rows { (rows - box_h) / 2 } else { 1 };

        g.clear().ok();
        g.set_cursor_position(0, start_y).ok();

        draw_top(g, &title, w);
        draw_empty(g, w);

        for (i, entry) in menu.entries.iter().enumerate() {
            draw_entry_line(g, entry, i, i == menu.selected, w);
        }

        draw_empty(g, w);
        draw_sep(g, w);

        if let Some(s) = &countdown {
            set_fg(g, Color::Yellow);
            let pad = inner.saturating_sub(s.chars().count());
            let left = pad / 2;
            let right = pad - left;
            write_str(g, &format!("│ {}{}{} │\r\n", " ".repeat(left), s, " ".repeat(right)));
        }

        set_fg(g, Color::DarkGray);
        write_str(g, &format!("│  {}{} │\r\n", foot1, " ".repeat(inner.saturating_sub(foot1.chars().count() + 2))));
        write_str(g, &format!("│  {}{} │\r\n", foot2, " ".repeat(inner.saturating_sub(foot2.chars().count() + 2))));
        draw_bottom(g, w);
    });
}

// ---------------------------------------------------------------------------
// Manual boot prompt
// ---------------------------------------------------------------------------

pub fn prompt_manual(input: &mut Input) -> Option<Entry> {
    uefi::system::with_stdout(|g| {
        g.clear().ok();
        let w = 54;
        let title = "Manual Boot".to_string();
        draw_top(g, &title, w);
        draw_empty(g, w);
        draw_line(g, "Enter path to .efi file", w, Color::White);
        draw_line(g, "(e.g. /EFI/arch/systemd-bootx64.efi)", w, Color::DarkGray);
        draw_empty(g, w);
        set_fg(g, Color::Cyan);
        write_str(g, &format!("│ {}│\r\n", " ".repeat(w.saturating_sub(2))));
        g.set_cursor_position(2, g.cursor_position().1.saturating_sub(1)).ok();
        write_str(g, "> ");
        set_fg(g, Color::White);
        draw_empty(g, w);
        draw_line(g, "Enter to boot, Esc to go back", w, Color::DarkGray);
        draw_bottom(g, w);
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
                    uefi::system::with_stdout(|g| {
                        let _ = g.output_string(cstr16!("\x08 \x08"));
                    });
                } else if (32..=126).contains(&c_val) {
                    buf.push(c_val as u8);
                    let pair = [c_val as u16, 0];
                    let _ = uefi::system::with_stdout(|g| {
                        if let Ok(cs) = uefi::CStr16::from_u16_with_nul(&pair) {
                            let _ = g.output_string(cs);
                        }
                    });
                }
            }
            Key::Special(sc) if sc == ScanCode::ESCAPE => return None,
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Key handling
// ---------------------------------------------------------------------------

fn poll_for_key(input: &mut Input, delay_ms: u64) -> Option<Key> {
    let iterations = delay_ms / 10;
    for _ in 0..iterations {
        if let Ok(Some(key)) = input.read_key() {
            return Some(key);
        }
        boot::stall(core::time::Duration::from_millis(10));
    }
    input.read_key().ok().flatten()
}

fn read_key_blocking(input: &mut Input) -> Option<Key> {
    loop {
        match input.read_key() {
            Ok(Some(key)) => return Some(key),
            Ok(None) => {}
            Err(_) => return None,
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
            if c_val == b'm' as u16 || c_val == b'M' as u16 || c_val == b',' as u16 {
                return KeyAction::Manual;
            }
            if c_val == b'b' as u16 || c_val == b'B' as u16 {
                return KeyAction::RestoreBackup;
            }
            if c_val == b'r' as u16 || c_val == b'R' as u16 {
                crate::boot_loader::reset_system();
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
