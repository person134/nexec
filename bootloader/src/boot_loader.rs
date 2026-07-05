use alloc::vec::Vec;
use uefi::boot;
use uefi::boot::{LoadImageSource, OpenProtocolAttributes, OpenProtocolParams, SearchType};
use uefi::fs::FileSystem;
use uefi::println;
use uefi::proto::console::text::Input;
use uefi::proto::device_path::build;
use uefi::proto::device_path::DevicePath;
use uefi::proto::loaded_image::LoadedImage;
use uefi::CString16;
use uefi::Identify;

use crate::config::Entry;
use crate::util;

pub fn boot_entry(entry: &Entry) -> bool {
    // Read the kernel file into memory and load via FromBuffer.
    let kernel_data = match read_efi_file(&entry.efi_path) {
        Ok(d) => d,
        Err(e) => {
            println!("Failed to read kernel {}: {}", entry.efi_path, e);
            return false;
        }
    };

    // Build a device path so the kernel sees the right device handle.
    let file_path = build_file_device_path(&entry.efi_path);

    let new_handle = match boot::load_image(
        boot::image_handle(),
        LoadImageSource::FromBuffer {
            buffer: &kernel_data,
            file_path: file_path.as_deref(),
        },
    ) {
        Ok(h) => h,
        Err(e) => {
            println!("LoadImage failed: {:?}", e.status());
            return false;
        }
    };
    drop(kernel_data);

    // Build kernel command line: options from config + initrd= on cmdline
    let mut cmdline = entry.options.clone().unwrap_or_default();
    for initrd_path in &entry.initrd {
        if !cmdline.is_empty() {
            cmdline.push(' ');
        }
        cmdline.push_str(&alloc::format!("initrd={}", initrd_path));
    }

    // Warn if no root=
    if !cmdline.split(' ').any(|p| p.starts_with("root=")) {
        println!("Warning: no root= in cmdline. UKIs embed one; raw kernels will panic.");
    }

    // Keep cmdline alive past start_image.
    // Kernel reads LoadOptions as UCS-2 (efi_char16_t), so encode as UTF-16.
    let mut _cmdline_storage: Option<Vec<u16>> = None;

    if !cmdline.is_empty() {
        let mut buf: Vec<u16> = cmdline.encode_utf16().collect();
        buf.push(0);
        if let Ok(mut loaded_image) = boot::open_protocol_exclusive::<LoadedImage>(new_handle) {
            unsafe {
                loaded_image.set_load_options(buf.as_ptr() as *const u8, (buf.len() * 2) as u32);
            }
        }
        _cmdline_storage = Some(buf);
    }

    match boot::start_image(new_handle) {
        Ok(()) => {
            println!("Booted image returned (unexpected). Press any key to reboot...");
            wait_for_key();
            reset_system();
        }
        Err(status) => {
            println!("Boot failed: {:?}", status);
            false
        }
    }
}

fn build_file_device_path(efi_path: &str) -> Option<uefi::proto::device_path::PoolDevicePath> {
    let normalized = util::normalize_path(efi_path);
    let target_cstr = CString16::try_from(normalized.as_str()).ok()?;
    let file_node = build::media::FilePath {
        path_name: &target_cstr,
    };

    let loaded_image = boot::open_protocol_exclusive::<LoadedImage>(boot::image_handle()).ok()?;
    let device_handle = loaded_image.device()?;
    let partition_dp = boot::open_protocol_exclusive::<DevicePath>(device_handle).ok()?;

    let mut tmp_vec = Vec::new();
    let tmp_dp = build::DevicePathBuilder::with_vec(&mut tmp_vec)
        .push(&file_node)
        .ok()?
        .finalize()
        .ok()?;
    let node = tmp_dp.node_iter().next()?;

    partition_dp.append_node(node).ok()
}

fn read_efi_file(path: &str) -> Result<alloc::vec::Vec<u8>, &'static str> {
    let sfsp =
        boot::get_image_file_system(boot::image_handle()).map_err(|_| "no filesystem")?;
    let mut fs = FileSystem::new(sfsp);
    let normalized = util::normalize_path(path);
    let cstr = CString16::try_from(normalized.as_str()).map_err(|_| "bad path")?;
    fs.read(cstr.as_ref()).map_err(|_| "read failed")
}

pub fn find_input() -> Result<(boot::ScopedProtocol<Input>, uefi::Handle), ()> {
    let raw_st = uefi::table::system_table_raw().ok_or(())?;
    let st = unsafe { raw_st.as_ref() };
    if !st.stdin.is_null() {
        let input = unsafe { &mut *(st.stdin.cast::<Input>()) };
        let _ = input.reset(false);
        if let Ok(handles) = boot::locate_handle_buffer(SearchType::ByProtocol(&Input::GUID)) {
            if let Some(h) = handles.iter().next() {
                if let Ok(g) = boot::open_protocol_exclusive::<Input>(*h) {
                    return Ok((g, *h));
                }
                if let Ok(g) = unsafe {
                    boot::open_protocol::<Input>(
                        OpenProtocolParams {
                            handle: *h,
                            agent: boot::image_handle(),
                            controller: None,
                        },
                        OpenProtocolAttributes::GetProtocol,
                    )
                } {
                    return Ok((g, *h));
                }
            }
        }
    }

    let handles = boot::locate_handle_buffer(SearchType::ByProtocol(&Input::GUID))
        .map_err(|_| ())?;
    for handle in handles.iter() {
        let mut guard = if let Ok(g) = boot::open_protocol_exclusive::<Input>(*handle) {
            g
        } else if let Ok(g) = unsafe {
            boot::open_protocol::<Input>(
                OpenProtocolParams {
                    handle: *handle,
                    agent: boot::image_handle(),
                    controller: None,
                },
                OpenProtocolAttributes::GetProtocol,
            )
        } {
            g
        } else {
            continue;
        };
        let _ = guard.reset(false);
        return Ok((guard, *handle));
    }
    Err(())
}

pub fn wait_for_key() {
    let raw_st = uefi::table::system_table_raw();
    let input: &mut Input = if let Some(st) = raw_st {
        let st = unsafe { st.as_ref() };
        if !st.stdin.is_null() {
            let input = unsafe { &mut *(st.stdin.cast::<Input>()) };
            let _ = input.reset(false);
            input
        } else {
            return;
        }
    } else {
        return;
    };
    loop {
        if let Ok(Some(_)) = input.read_key() {
            return;
        }
        boot::stall(core::time::Duration::from_millis(10));
    }
}

pub fn reset_system() -> ! {
    uefi::runtime::reset(uefi::runtime::ResetType::COLD, uefi::Status::SUCCESS, None)
}
