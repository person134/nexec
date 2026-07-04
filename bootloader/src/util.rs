/// Normalize a path for UEFI filesystem APIs: convert `/` to `\`.
/// UEFI firmware expects backslashes as path separators.
pub fn normalize_path(path: &str) -> alloc::string::String {
    path.replace('/', "\\")
}
