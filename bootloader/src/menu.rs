use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::ops::{Deref, DerefMut};
use uefi::boot;
use uefi::boot::{OpenProtocolAttributes, OpenProtocolParams, SearchType};
use uefi::proto::console::text::{Color, Input, Key, Output, ScanCode};
use uefi::CStr16;
use uefi::Identify;

use crate::config::{Config, Entry};
use crate::detect;

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

fn set_fg_bg(g: &mut Output, fg: Color, bg: Color) {
    let _ = g.set_color(fg, bg);
}

// ---------------------------------------------------------------------------
// Box-drawing primitives (each writes one line, ends with \r\n)
// ---------------------------------------------------------------------------

fn box_width(cols: usize, title: &str, lines: &[String]) -> usize {
    let entry_max = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0);
    let f1 = "↑↓=boot  m=manual  b=backups";
    let f2 = "f=firmware  r=reboot";
    let max_content = title.chars().count().max(entry_max).max(f1.len()).max(f2.len());
    let desired = max_content + 6;
    ((cols as usize).saturating_sub(2)).min(52.max(desired))
}

fn indent(g: &mut Output, x: usize) {
    if x > 0 {
        write_str(g, &" ".repeat(x));
    }
}

fn draw_top(g: &mut Output, title: &str, w: usize, x: usize) {
    set_fg_bg(g, Color::White, Color::Black);
    let dashes = "─".repeat(w.saturating_sub(title.len() + 4));
    indent(g, x);
    write_str(g, &format!("┌ {} {}┐\r\n", title, dashes));
}

fn draw_bottom(g: &mut Output, w: usize, x: usize) {
    set_fg_bg(g, Color::White, Color::Black);
    indent(g, x);
    write_str(g, &format!("└{}┘\r\n", "─".repeat(w.saturating_sub(2))));
}

fn draw_empty(g: &mut Output, w: usize, x: usize) {
    set_fg_bg(g, Color::White, Color::Black);
    indent(g, x);
    write_str(g, &format!("│{}│\r\n", " ".repeat(w.saturating_sub(2))));
}

fn draw_sep(g: &mut Output, w: usize, x: usize) {
    let inner = w.saturating_sub(4);
    set_fg_bg(g, Color::White, Color::Black);
    indent(g, x);
    write_str(g, "│ ");
    set_fg_bg(g, Color::DarkGray, Color::Black);
    write_str(g, &"─".repeat(inner));
    set_fg_bg(g, Color::White, Color::Black);
    write_str(g, " │\r\n");
}

fn draw_line(g: &mut Output, text: &str, w: usize, color: Color, x: usize) {
    let inner = w.saturating_sub(4);
    let tlen = text.chars().count();
    let display: String = text.chars().take(inner).collect();
    set_fg_bg(g, Color::White, Color::Black);
    indent(g, x);
    write_str(g, "│ ");
    set_fg_bg(g, color, Color::Black);
    write_str(g, &display);
    if tlen < inner {
        write_str(g, &" ".repeat(inner - tlen));
    }
    set_fg_bg(g, Color::White, Color::Black);
    write_str(g, " │\r\n");
}

fn draw_entry_line(g: &mut Output, entry: &Entry, selected: bool, w: usize, x: usize) {
    let prefix = if selected { "  > " } else { "    " };
    let counter = entry.boot_counter.map_or(String::new(), |c| {
        if c > 0 { format!(" [{}]", c) } else { String::new() }
    });

    let inner = w.saturating_sub(4);
    let counter_len = counter.chars().count();
    let max_title = inner.saturating_sub(4 + counter_len);
    let title_display: String = entry.title.chars().take(max_title).collect();
    let tlen = title_display.chars().count();
    let pad = max_title.saturating_sub(tlen);
    let left = pad / 2;
    let right = pad - left;

    let bg = if selected { Color::LightGray } else { Color::Black };

    set_fg_bg(g, Color::White, Color::Black);
    indent(g, x);
    write_str(g, "│ ");
    set_fg_bg(g, Color::White, bg);
    write_str(g, prefix);
    write_str(g, &" ".repeat(left));
    write_str(g, &title_display);
    write_str(g, &" ".repeat(right));

    if !counter.is_empty() {
        set_fg_bg(g, Color::DarkGray, bg);
        write_str(g, &counter);
    }

    set_fg_bg(g, Color::White, Color::Black);
    write_str(g, " │\r\n");
}

// ---------------------------------------------------------------------------
// Public: show a status/info dialog (centered, monochrome)
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
        let start_x = (cols - w) / 2;

        g.set_cursor_position(0, start_y).ok();

        draw_top(g, &title, w, start_x);
        draw_empty(g, w, start_x);
        for (text, color) in lines {
            draw_line(g, text, w, *color, start_x);
        }
        draw_empty(g, w, start_x);
        draw_bottom(g, w, start_x);
        // Clear any ghost from a different-height previous draw
        set_fg_bg(g, Color::Black, Color::Black);
        indent(g, start_x);
        write_str(g, &" ".repeat(w));
        set_fg_bg(g, Color::White, Color::Black);
        write_str(g, "\r\n");
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

        let mut dirty = true;

        loop {
            if self.entries.is_empty() {
                draw_no_entries();
                match browse_efi_files(input) {
                    Some(entry) => return MenuResult::Boot(entry),
                    None => continue,
                }
            }

            if dirty {
                draw_menu(self, remaining);
                dirty = false;
            }

            if remaining > 0 {
                if let Some(k) = poll_for_key(input, 100) {
                    remaining = 0;
                    dirty = true;
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
                let prev = remaining / 10;
                remaining = remaining.saturating_sub(1);
                if remaining / 10 != prev || remaining == 0 {
                    dirty = true;
                }
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
        let (cols, rows) = g.current_mode().ok().flatten()
            .map(|m| (m.columns(), m.rows()))
            .unwrap_or((80, 25));
        let w = ((cols as usize).saturating_sub(2)).min(46);
        let box_h = 8;
        let start_y = if box_h < rows { (rows - box_h) / 2 } else { 1 };
        let start_x = (cols - w) / 2;
        let title = format!("hboot v{}", VERSION);

        g.set_cursor_position(0, start_y).ok();

        draw_top(g, &title, w, start_x);
        draw_empty(g, w, start_x);
        draw_line(g, "No entries detected", w, Color::White, start_x);
        draw_empty(g, w, start_x);
        draw_line(g, "m  manual boot", w, Color::DarkGray, start_x);
        draw_line(g, "f  firmware setup", w, Color::DarkGray, start_x);
        draw_empty(g, w, start_x);
        draw_bottom(g, w, start_x);
        set_fg_bg(g, Color::Black, Color::Black);
        indent(g, start_x);
        write_str(g, &" ".repeat(w));
        set_fg_bg(g, Color::White, Color::Black);
        write_str(g, "\r\n");
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
        for e in &menu.entries {
            let counter = e.boot_counter.map_or(String::new(), |c| {
                if c > 0 { format!(" [{}]", c) } else { String::new() }
            });
            lines.push(format!("    {}{}", e.title, counter));
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
        let start_x = (cols - w) / 2;

        g.set_cursor_position(0, start_y).ok();

        draw_top(g, &title, w, start_x);
        draw_empty(g, w, start_x);

        for (i, entry) in menu.entries.iter().enumerate() {
            draw_entry_line(g, entry, i == menu.selected, w, start_x);
        }

        draw_empty(g, w, start_x);
        draw_sep(g, w, start_x);

        if let Some(s) = &countdown {
            let pad = inner.saturating_sub(s.chars().count());
            let left = pad / 2;
            let right = pad - left;
            indent(g, start_x);
            set_fg_bg(g, Color::White, Color::Black);
            write_str(g, "│ ");
            set_fg_bg(g, Color::White, Color::Black);
            write_str(g, &" ".repeat(left));
            write_str(g, s);
            write_str(g, &" ".repeat(right));
            set_fg_bg(g, Color::White, Color::Black);
            write_str(g, " │\r\n");
        }

        indent(g, start_x);
        set_fg_bg(g, Color::White, Color::Black);
        write_str(g, "│ ");
        set_fg_bg(g, Color::White, Color::Black);
        write_str(g, " ");
        write_str(g, foot1);
        write_str(g, &" ".repeat(inner.saturating_sub(foot1.chars().count() + 1)));
        set_fg_bg(g, Color::White, Color::Black);
        write_str(g, " │\r\n");

        indent(g, start_x);
        set_fg_bg(g, Color::White, Color::Black);
        write_str(g, "│ ");
        set_fg_bg(g, Color::White, Color::Black);
        write_str(g, " ");
        write_str(g, foot2);
        write_str(g, &" ".repeat(inner.saturating_sub(foot2.chars().count() + 1)));
        set_fg_bg(g, Color::White, Color::Black);
        write_str(g, " │\r\n");

        draw_bottom(g, w, start_x);
        set_fg_bg(g, Color::Black, Color::Black);
        indent(g, start_x);
        write_str(g, &" ".repeat(w));
        set_fg_bg(g, Color::White, Color::Black);
        write_str(g, "\r\n");
    });
}

fn azerty_to_qwerty(c: u16) -> u16 {
    match c {
        113 => 97,   // q → a
        81  => 65,   // Q → A
        97  => 113,  // a → q
        65  => 81,   // A → Q
        119 => 122,  // w → z
        87  => 90,   // W → Z
        122 => 119,  // z → w
        90  => 87,   // Z → W
        44  => 109,  // , → m
        109 => 44,   // m → ,
        _ => c,
    }
}

// ---------------------------------------------------------------------------
// EFI file browser (replaces manual text-input prompt)
// ---------------------------------------------------------------------------

pub fn browse_efi_files(input: &mut Input) -> Option<Entry> {
    let entries = detect::scan_efi_files();
    if entries.is_empty() {
        show_status(&[
            ("No .efi files found on ESP", Color::White),
            ("Press any key to go back...", Color::DarkGray),
        ]);
        let _ = input.reset(false);
        loop {
            if let Some(_key) = read_key_blocking(input) {
                break;
            }
        }
        return None;
    }

    let mut selected = 0;

    loop {
        uefi::system::with_stdout(|g| {
            let (cols, rows) = g
                .current_mode()
                .ok()
                .flatten()
                .map(|m| (m.columns(), m.rows()))
                .unwrap_or((80, 25));

            let title = "Manual Boot — Select .efi";
            let lines: Vec<String> = entries
                .iter()
                .map(|e| e.efi_path.clone())
                .collect();
            let w = box_width(cols, title, &lines);
            let box_h = entries.len() + 6;
            let start_y = if box_h < rows { (rows - box_h) / 2 } else { 1 };
            let start_x = (cols - w) / 2;

            g.set_cursor_position(0, start_y).ok();

            draw_top(g, title, w, start_x);
            draw_empty(g, w, start_x);

            for (i, entry) in entries.iter().enumerate() {
                draw_entry_line(g, entry, i == selected, w, start_x);
            }

            draw_empty(g, w, start_x);
            draw_line(g, "Enter=boot  Esc=back", w, Color::DarkGray, start_x);
            draw_bottom(g, w, start_x);
            set_fg_bg(g, Color::Black, Color::Black);
            indent(g, start_x);
            write_str(g, &" ".repeat(w));
            set_fg_bg(g, Color::White, Color::Black);
            write_str(g, "\r\n");
        });

        let Some(key) = read_key_blocking(input) else { continue };
        match key {
            Key::Special(sc) if sc == ScanCode::UP => {
                if selected > 0 {
                    selected -= 1;
                }
            }
            Key::Special(sc) if sc == ScanCode::DOWN => {
                if selected < entries.len().saturating_sub(1) {
                    selected += 1;
                }
            }
            Key::Special(sc) if sc == ScanCode::HOME => {
                selected = 0;
            }
            Key::Special(sc) if sc == ScanCode::END => {
                selected = entries.len().saturating_sub(1);
            }
            Key::Printable(c) => {
                let c_val: u16 = c.into();
                if c_val == b'\r' as u16 || c_val == b'\n' as u16 {
                    return Some(entries[selected].clone());
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
            let c_val: u16 = azerty_to_qwerty(c.into());
            if (c_val == b'\r' as u16 || c_val == b'\n' as u16) && !menu.entries.is_empty() {
                return KeyAction::Boot;
            }
            if c_val == b'f' as u16 || c_val == b'F' as u16 {
                boot_firmware();
            }
            if c_val == b'm' as u16 || c_val == b'M' as u16 {
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
