#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::string::String;
use alloc::string::ToString;
use alloc::vec::Vec;

#[derive(Debug, Clone)]
pub struct Entry {
    pub name: String,
    pub title: String,
    pub efi_path: String,
    pub options: Option<String>,
    pub initrd: Option<String>,
    /// Boot counter: None = no counting, Some(0) = exhausted, Some(N) = N tries left.
    /// Stored in an external state file, not parsed from entry content.
    pub boot_counter: Option<u32>,
    /// Filesystem path to the entry file (used for boot-count file writes).
    pub source_path: Option<String>,
}

#[derive(Debug)]
pub struct Config {
    pub default: Option<String>,
    pub timeout: u64,
    pub no_scan: bool,
    pub order: Option<Vec<String>>,
    pub entries: Vec<Entry>,
}

impl Config {
    /// Parse an entry file (no `[name]` header — name comes from the filename).
    /// Fields: title, efi, options, initrd.
    pub fn parse_entry_file(name: &str, data: &[u8]) -> Result<Entry, &'static str> {
        let text = core::str::from_utf8(data).map_err(|_| "entry file not valid UTF-8")?;
        let mut entry = Entry {
            name: name.to_string(),
            title: String::new(),
            efi_path: String::new(),
            options: None,
            initrd: None,
            boot_counter: None,
            source_path: None,
        };
        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            if let Some(eq) = trimmed.find('=') {
                let key = trimmed[..eq].trim();
                let value = trimmed[eq + 1..].trim();
                let value = value.trim_matches('"');
                match key {
                    "title" => entry.title = value.to_string(),
                    "efi" => entry.efi_path = value.to_string(),
                    "options" => {
                        entry.options = if value.is_empty() { None } else { Some(value.to_string()) }
                    }
                    "initrd" => {
                        entry.initrd = if value.is_empty() { None } else { Some(value.to_string()) }
                    }
                    _ => {}
                }
            }
        }
        if entry.efi_path.is_empty() {
            return Err("entry file has no efi = path");
        }
        Ok(entry)
    }

    /// Parse a Boot Loader Spec (type-1) entry file.
    /// Keys: title, linux (→ efi), initrd, options, efi (direct).
    /// Paths use `/` separators; they are converted to `\` for hboot.
    /// The entry name is derived from the filename (without extension).
    pub fn parse_bls_entry(filename: &str, data: &[u8]) -> Result<Entry, &'static str> {
        let text = core::str::from_utf8(data).map_err(|_| "BLS entry not valid UTF-8")?;
        let mut entry = Entry {
            name: filename.to_string(),
            title: String::new(),
            efi_path: String::new(),
            options: None,
            initrd: None,
            boot_counter: None,
            source_path: None,
        };

        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            if let Some(eq) = trimmed.find(char::is_whitespace) {
                let key = trimmed[..eq].trim();
                let value = trimmed[eq + 1..].trim();
                let value = value.trim_matches('"');
                // BLS uses / paths; convert to \ for hboot
                let converted = value.replace('/', "\\");
                match key {
                    "title" => entry.title = value.to_string(),
                    "linux" => entry.efi_path = converted,
                    "initrd" => entry.initrd = Some(converted),
                    "options" => {
                        entry.options = if value.is_empty() { None } else { Some(value.to_string()) }
                    }
                    "efi" => entry.efi_path = converted,
                    _ => {}
                }
            }
        }

        if entry.efi_path.is_empty() {
            return Err("BLS entry has no linux= or efi= path");
        }
        Ok(entry)
    }

    pub fn parse(data: &[u8]) -> Result<Self, &'static str> {
        let text = core::str::from_utf8(data).map_err(|_| "config not valid UTF-8")?;
        let mut config = Config {
            default: None,
            timeout: 5,
            no_scan: false,
            order: None,
            entries: Vec::new(),
        };
        let mut current: Option<Entry> = None;

        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }

            if trimmed.starts_with('[') {
                if let Some(entry) = current.take() {
                    config.entries.push(entry);
                }
                let end = trimmed.find(']').ok_or("unclosed section")?;
                let name = trimmed[1..end].trim();
                if name.is_empty() {
                    return Err("empty section name");
                }
                current = Some(Entry {
                    name: name.to_string(),
                    title: String::new(),
                    efi_path: String::new(),
                    options: None,
                    initrd: None,
                    boot_counter: None,
                    source_path: None,
                });
                continue;
            }

            if let Some(eq) = trimmed.find('=') {
                let key = trimmed[..eq].trim();
                let value = trimmed[eq + 1..].trim();
                let value = value.trim_matches('"');

                if let Some(ref mut entry) = current {
                    match key {
                        "title" => entry.title = value.to_string(),
                        "efi" => entry.efi_path = value.to_string(),
                        "options" => {
                            entry.options = if value.is_empty() {
                                None
                            } else {
                                Some(value.to_string())
                            }
                        }
                        "initrd" => {
                            entry.initrd = if value.is_empty() {
                                None
                            } else {
                                Some(value.to_string())
                            }
                        }
                        _ => {}
                    }
                } else {
                    match key {
                        "default" => config.default = Some(value.to_string()),
                        "timeout" => {
                            config.timeout = value.parse().map_err(|_| "invalid timeout")?
                        }
                        "no_scan" => {
                            config.no_scan = matches!(value.to_lowercase().as_str(), "true" | "yes" | "1");
                        }
                        "scan" => {
                            config.no_scan = !matches!(value.to_lowercase().as_str(), "true" | "yes" | "1");
                        }
                        "order" => {
                            config.order = Some(
                                value.split_whitespace().map(|s| s.to_string()).collect(),
                            );
                        }
                        _ => {}
                    }
                }
            }
        }

        if let Some(entry) = current.take() {
            config.entries.push(entry);
        }

        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> Result<Config, &'static str> {
        Config::parse(s.as_bytes())
    }

    #[test]
    fn full_config() {
        let data = r#"
default = arch
timeout = 3
order = arch windows

[arch]
title = Arch Linux
efi = \vmlinuz-linux
options = root=UUID=123 rw quiet
initrd = \initramfs-linux.img

[windows]
title = Windows
efi = \EFI\Microsoft\Boot\bootmgfw.efi
"#;
        let cfg = parse(data).unwrap();
        assert_eq!(cfg.default.as_deref(), Some("arch"));
        assert_eq!(cfg.timeout, 3);
        assert!(!cfg.no_scan);
        assert_eq!(cfg.order.as_deref().unwrap(), &["arch", "windows"]);
        assert_eq!(cfg.entries.len(), 2);

        assert_eq!(cfg.entries[0].name, "arch");
        assert_eq!(cfg.entries[0].title, "Arch Linux");
        assert_eq!(cfg.entries[0].efi_path, "\\vmlinuz-linux");
        assert_eq!(cfg.entries[0].options.as_deref(), Some("root=UUID=123 rw quiet"));
        assert_eq!(cfg.entries[0].initrd.as_deref(), Some("\\initramfs-linux.img"));

        assert_eq!(cfg.entries[1].name, "windows");
        assert_eq!(cfg.entries[1].title, "Windows");
        assert_eq!(cfg.entries[1].efi_path, "\\EFI\\Microsoft\\Boot\\bootmgfw.efi");
        assert_eq!(cfg.entries[1].options, None);
        assert_eq!(cfg.entries[1].initrd, None);
    }

    #[test]
    fn empty_config() {
        let cfg = parse("").unwrap();
        assert!(cfg.default.is_none());
        assert_eq!(cfg.timeout, 5);
        assert!(!cfg.no_scan);
        assert!(cfg.order.is_none());
        assert!(cfg.entries.is_empty());
    }

    #[test]
    fn comments_only() {
        let cfg = parse("# default = foo\n# timeout = 10").unwrap();
        assert!(cfg.default.is_none());
        assert_eq!(cfg.timeout, 5);
    }

    #[test]
    fn no_section_name() {
        let result = parse("[]\ntitle = foo");
        assert_eq!(result.err(), Some("empty section name"));
    }

    #[test]
    fn unclosed_section() {
        let result = parse("[foo\ntitle = bar");
        assert_eq!(result.err(), Some("unclosed section"));
    }

    #[test]
    fn not_utf8() {
        let result = Config::parse(&[0xFF, 0xFE, 0x00]);
        assert_eq!(result.err(), Some("config not valid UTF-8"));
    }

    #[test]
    fn invalid_timeout() {
        let result = parse("timeout = not_a_number");
        assert_eq!(result.err(), Some("invalid timeout"));
    }

    #[test]
    fn timeout_zero() {
        let cfg = parse("timeout = 0").unwrap();
        assert_eq!(cfg.timeout, 0);
    }

    #[test]
    fn no_scan_true() {
        let cfg = parse("no_scan = true").unwrap();
        assert!(cfg.no_scan);
    }

    #[test]
    fn no_scan_yes() {
        let cfg = parse("no_scan = yes").unwrap();
        assert!(cfg.no_scan);
    }

    #[test]
    fn no_scan_1() {
        let cfg = parse("no_scan = 1").unwrap();
        assert!(cfg.no_scan);
    }

    #[test]
    fn no_scan_false() {
        let cfg = parse("no_scan = false").unwrap();
        assert!(!cfg.no_scan);
    }

    #[test]
    fn scan_directive() {
        let cfg = parse("scan = false").unwrap();
        assert!(cfg.no_scan);
    }

    #[test]
    fn scan_true() {
        let cfg = parse("scan = true").unwrap();
        assert!(!cfg.no_scan);
    }

    #[test]
    fn order_with_spaces() {
        let cfg = parse("order = foo bar baz").unwrap();
        assert_eq!(cfg.order.as_deref().unwrap(), &["foo", "bar", "baz"]);
    }

    #[test]
    fn quoted_values() {
        let data = r#"[test]
title = "My Title"
efi = "\EFI\path\file.efi""#;
        let cfg = parse(data).unwrap();
        assert_eq!(cfg.entries[0].title, "My Title");
    }

    #[test]
    fn single_entry_minimal() {
        let data = "[mine]\nefi = \\EFI\\test.efi";
        let cfg = parse(data).unwrap();
        assert_eq!(cfg.entries.len(), 1);
        assert_eq!(cfg.entries[0].name, "mine");
        assert_eq!(cfg.entries[0].title, "");
        assert_eq!(cfg.entries[0].efi_path, "\\EFI\\test.efi");
        assert_eq!(cfg.entries[0].options, None);
        assert_eq!(cfg.entries[0].initrd, None);
    }

    #[test]
    fn multiple_entries() {
        let data = "[one]\nefi = a\n[two]\nefi = b\n[three]\nefi = c";
        let cfg = parse(data).unwrap();
        assert_eq!(cfg.entries.len(), 3);
        assert_eq!(cfg.entries[0].name, "one");
        assert_eq!(cfg.entries[1].name, "two");
        assert_eq!(cfg.entries[2].name, "three");
    }

    #[test]
    fn empty_options_and_initrd() {
        let data = "[test]\ntitle = Test\nefi = x\noptions = \ninitrd = ";
        let cfg = parse(data).unwrap();
        assert_eq!(cfg.entries[0].options, None);
        assert_eq!(cfg.entries[0].initrd, None);
    }

    #[test]
    fn unknown_global_key() {
        let cfg = parse("unknown_key = whatever").unwrap();
        assert_eq!(cfg.timeout, 5);
    }

    #[test]
    fn unknown_entry_key() {
        let data = "[test]\nefi = x\ncolor = blue";
        let cfg = parse(data).unwrap();
        assert_eq!(cfg.entries[0].efi_path, "x");
    }

    #[test]
    fn default_only() {
        let cfg = parse("default = foo").unwrap();
        assert_eq!(cfg.default.as_deref(), Some("foo"));
        assert!(cfg.entries.is_empty());
    }

    #[test]
    fn defaults_when_missing() {
        let cfg = parse("[e]\nefi = p").unwrap();
        assert!(cfg.default.is_none());
        assert_eq!(cfg.timeout, 5);
        assert!(!cfg.no_scan);
        assert!(cfg.order.is_none());
    }

    #[test]
    fn carriage_return_lines() {
        let data = "default = a\r\ntimeout = 7\r\n[e]\r\nefi = p\r\n";
        let cfg = parse(data).unwrap();
        assert_eq!(cfg.default.as_deref(), Some("a"));
        assert_eq!(cfg.timeout, 7);
        assert_eq!(cfg.entries[0].efi_path, "p");
    }

    #[test]
    fn section_name_whitespace() {
        let data = "[  my entry  ]\nefi = p";
        let cfg = parse(data).unwrap();
        assert_eq!(cfg.entries[0].name, "my entry");
    }

    #[test]
    fn value_with_equals() {
        let data = r#"[e]
options = root=UUID=abc rw"#;
        let cfg = parse(data).unwrap();
        assert_eq!(cfg.entries[0].options.as_deref(), Some("root=UUID=abc rw"));
    }

    #[test]
    fn no_entries() {
        let data = "default = foo\ntimeout = 10\norder = a b";
        let cfg = parse(data).unwrap();
        assert!(cfg.entries.is_empty());
        assert_eq!(cfg.default.as_deref(), Some("foo"));
        assert_eq!(cfg.timeout, 10);
    }

    #[test]
    fn default_overrides() {
        let cfg = parse("default = second\n[e1]\nefi = a\n[e2]\nefi = b").unwrap();
        assert_eq!(cfg.default.as_deref(), Some("second"));
        assert_eq!(cfg.entries.len(), 2);
    }

    #[test]
    fn all_keys_in_entry() {
        let data = r#"[arch]
title = Arch Linux
efi = \vmlinuz-linux
options = root=PARTUUID=abc rw
initrd = \initramfs-linux.img"#;
        let cfg = parse(data).unwrap();
        let e = &cfg.entries[0];
        assert_eq!(e.name, "arch");
        assert_eq!(e.title, "Arch Linux");
        assert_eq!(e.efi_path, "\\vmlinuz-linux");
        assert_eq!(e.options.as_deref(), Some("root=PARTUUID=abc rw"));
        assert_eq!(e.initrd.as_deref(), Some("\\initramfs-linux.img"));
    }

    #[test]
    fn parse_entry_file_full() {
        let data = "title = My Distro\nefi = \\vmlinuz-6.1.0\noptions = root=UUID=abc rw\ninitrd = \\initramfs-6.1.0.img\n";
        let e = Config::parse_entry_file("mydistro", data.as_bytes()).unwrap();
        assert_eq!(e.name, "mydistro");
        assert_eq!(e.title, "My Distro");
        assert_eq!(e.efi_path, "\\vmlinuz-6.1.0");
        assert_eq!(e.options.as_deref(), Some("root=UUID=abc rw"));
        assert_eq!(e.initrd.as_deref(), Some("\\initramfs-6.1.0.img"));
    }

    #[test]
    fn parse_entry_file_minimal() {
        let data = "efi = \\vmlinuz-linux";
        let e = Config::parse_entry_file("linux", data.as_bytes()).unwrap();
        assert_eq!(e.name, "linux");
        assert_eq!(e.title, "");
        assert_eq!(e.efi_path, "\\vmlinuz-linux");
        assert!(e.options.is_none());
        assert!(e.initrd.is_none());
    }

    #[test]
    fn parse_entry_file_no_efi() {
        let data = "title = Broken";
        let result = Config::parse_entry_file("broken", data.as_bytes());
        assert!(result.is_err());
    }

    #[test]
    fn parse_entry_file_comments_and_blanks() {
        let data = "# comment\ntitle = Test\n\nefi = \\test.efi\n";
        let e = Config::parse_entry_file("test", data.as_bytes()).unwrap();
        assert_eq!(e.title, "Test");
        assert_eq!(e.efi_path, "\\test.efi");
    }

    #[test]
    fn parse_entry_file_quoted_values() {
        let data = "title = \"My OS\"\nefi = \"\\path\\to\\kernel.efi\"";
        let e = Config::parse_entry_file("os", data.as_bytes()).unwrap();
        assert_eq!(e.title, "My OS");
        assert_eq!(e.efi_path, "\\path\\to\\kernel.efi");
    }

    // ---- BLS entry parsing tests ----

    fn parse_bls(filename: &str, data: &str) -> Result<Entry, &'static str> {
        Config::parse_bls_entry(filename, data.as_bytes())
    }

    #[test]
    fn bls_full_entry() {
        let data = r#"title Arch Linux
linux /vmlinuz-linux
initrd /initramfs-linux.img
options root=UUID=123 rw quiet
"#;
        let e = parse_bls("arch", data).unwrap();
        assert_eq!(e.name, "arch");
        assert_eq!(e.title, "Arch Linux");
        assert_eq!(e.efi_path, "\\vmlinuz-linux");
        assert_eq!(e.initrd.as_deref(), Some("\\initramfs-linux.img"));
        assert_eq!(e.options.as_deref(), Some("root=UUID=123 rw quiet"));
    }

    #[test]
    fn bls_minimal_linux() {
        let data = "linux /vmlinuz-linux";
        let e = parse_bls("linux", data).unwrap();
        assert_eq!(e.name, "linux");
        assert_eq!(e.efi_path, "\\vmlinuz-linux");
        assert!(e.initrd.is_none());
        assert!(e.options.is_none());
    }

    #[test]
    fn bls_efi_direct() {
        let data = "efi /EFI/Linux/arch.efi";
        let e = parse_bls("arch.efi", data).unwrap();
        assert_eq!(e.efi_path, "\\EFI\\Linux\\arch.efi");
    }

    #[test]
    fn bls_linux_takes_precedence_over_efi() {
        let data = "efi /fallback.efi\nlinux /vmlinuz-linux";
        let e = parse_bls("test", data).unwrap();
        assert_eq!(e.efi_path, "\\vmlinuz-linux");
    }

    #[test]
    fn bls_no_efi_or_linux() {
        let data = "title Broken\noptions quiet";
        let result = parse_bls("broken", data);
        assert_eq!(result.err(), Some("BLS entry has no linux= or efi= path"));
    }

    #[test]
    fn bls_quoted_values() {
        let data = r#"title "Arch Linux"
linux "/vmlinuz-linux""#;
        let e = parse_bls("arch", data).unwrap();
        assert_eq!(e.title, "Arch Linux");
        assert_eq!(e.efi_path, "\\vmlinuz-linux");
    }

    #[test]
    fn bls_comment_and_blank_lines() {
        let data = "# comment\nlinux /vmlinuz-linux\n\ntitle Test\n";
        let e = parse_bls("test", data).unwrap();
        assert_eq!(e.title, "Test");
        assert_eq!(e.efi_path, "\\vmlinuz-linux");
    }

    #[test]
    fn bls_unknown_key() {
        let data = "linux /vmlinuz\nunknown = whatever\n";
        let e = parse_bls("test", data).unwrap();
        assert_eq!(e.efi_path, "\\vmlinuz");
    }

    #[test]
    fn bls_empty_options() {
        let data = "linux /vmlinuz\noptions ";
        let e = parse_bls("test", data).unwrap();
        assert!(e.options.is_none());
    }

    #[test]
    fn bls_not_utf8() {
        let result = Config::parse_bls_entry("bad", &[0xFF, 0xFE]);
        assert_eq!(result.err(), Some("BLS entry not valid UTF-8"));
    }

    #[test]
    fn bls_backslash_paths_preserved() {
        let data = "linux \\EFI\\linux\\kernel.efi";
        let e = parse_bls("test", data).unwrap();
        assert_eq!(e.efi_path, "\\EFI\\linux\\kernel.efi");
    }

    #[test]
    fn bls_filename_used_as_name() {
        let data = "linux /vmlinuz";
        let e = parse_bls("my-custom-entry", data).unwrap();
        assert_eq!(e.name, "my-custom-entry");
    }

    // ---- Config.parse edge cases ----

    #[test]
    fn config_with_scan_false() {
        let cfg = parse("scan = false").unwrap();
        assert!(cfg.no_scan);
    }

    #[test]
    fn config_with_scan_no() {
        let cfg = parse("scan = no").unwrap();
        assert!(cfg.no_scan);
    }

    #[test]
    fn no_scan_false_like() {
        let cfg = parse("no_scan = 0").unwrap();
        assert!(!cfg.no_scan);
    }

    #[test]
    fn no_scan_no() {
        let cfg = parse("no_scan = no").unwrap();
        assert!(!cfg.no_scan);
    }

    #[test]
    fn no_scan_no_case() {
        let cfg = parse("no_scan = True").unwrap();
        assert!(cfg.no_scan);
    }

    #[test]
    fn parse_entry_file_blank_lines_between_keys() {
        let data = "title = Test\n\nefi = \\test.efi\n\noptions = quiet\n";
        let e = Config::parse_entry_file("t", data.as_bytes()).unwrap();
        assert_eq!(e.title, "Test");
        assert_eq!(e.efi_path, "\\test.efi");
        assert_eq!(e.options.as_deref(), Some("quiet"));
    }

    #[test]
    fn parse_entry_file_extra_whitespace_around_equals() {
        let data = "title   =   My Entry\nefi\t=\t\\path.efi";
        let e = Config::parse_entry_file("e", data.as_bytes()).unwrap();
        assert_eq!(e.title, "My Entry");
        assert_eq!(e.efi_path, "\\path.efi");
    }

    #[test]
    fn global_keys_before_sections_only() {
        // Keys after a section header are treated as entry keys, not global
        let data = "default = e\n[e]\nefi = p";
        let cfg = parse(data).unwrap();
        assert_eq!(cfg.default.as_deref(), Some("e"));
        assert_eq!(cfg.entries.len(), 1);
    }

    #[test]
    fn parse_order_before_sections() {
        let data = "order = b a\n[a]\nefi = a\n[b]\nefi = b";
        let cfg = parse(data).unwrap();
        assert_eq!(cfg.order.as_deref().unwrap(), &["b", "a"]);
    }

    #[test]
    fn parse_trailing_whitespace_trimmed() {
        let data = "[test]\ntitle = My OS   \nefi = \\path.efi   ";
        let cfg = parse(data).unwrap();
        assert_eq!(cfg.entries[0].title, "My OS");
    }


}
