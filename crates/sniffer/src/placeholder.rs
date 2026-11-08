//! Online-only files (OneDrive, iCloud Drive, Dropbox). Reading one would
//! download it, so the sniffer must check metadata only (spec section 6).

use std::fs::Metadata;

#[cfg(windows)]
pub fn is_cloud_placeholder(meta: &Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_OFFLINE: u32 = 0x0000_1000;
    const FILE_ATTRIBUTE_RECALL_ON_OPEN: u32 = 0x0004_0000;
    const FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS: u32 = 0x0040_0000;
    meta.file_attributes()
        & (FILE_ATTRIBUTE_OFFLINE
            | FILE_ATTRIBUTE_RECALL_ON_OPEN
            | FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS)
        != 0
}

#[cfg(target_os = "macos")]
pub fn is_cloud_placeholder(meta: &Metadata) -> bool {
    use std::os::macos::fs::MetadataExt;
    // SF_DATALESS from <sys/stat.h>: the file's data lives with a file provider.
    const SF_DATALESS: u32 = 0x4000_0000;
    meta.st_flags() & SF_DATALESS != 0
}

#[cfg(not(any(windows, target_os = "macos")))]
pub fn is_cloud_placeholder(_meta: &Metadata) -> bool {
    false
}
