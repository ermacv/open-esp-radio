//! ESP-IDF application descriptor ABI, expressed as data rather than startup code.
//!
//! SOURCE: ESP-IDF 4d80c1b29aeabc8e735ffc6e1be9bb23e95d1d5d,
//! `components/esp_app_format/include/esp_app_desc.h:21-46`:
//! https://github.com/espressif/esp-idf/blob/4d80c1b29aeabc8e735ffc6e1be9bb23e95d1d5d/components/esp_app_format/include/esp_app_desc.h

#[derive(Debug, Eq, PartialEq)]
pub enum DescriptorError {
    Text,
    PageSize,
}

#[repr(C)]
pub struct AppDescriptor {
    magic: u32,
    secure_version: u32,
    reserved1: [u32; 2],
    version: [u8; 32],
    project: [u8; 32],
    time: [u8; 16],
    date: [u8; 16],
    idf_version: [u8; 32],
    elf_sha256: [u8; 32],
    min_revision: u16,
    max_revision: u16,
    mmu_page_log2: u8,
    flash_mode: u8,
    reserved3: [u8; 2],
    reserved2: [u32; 18],
}

impl AppDescriptor {
    /// Deterministic build metadata, including the ELF hash slot.
    /// Text must fit with its terminating NUL and contain no interior NUL.
    pub const fn new(
        version: &str,
        project: &str,
        page_bytes: u32,
    ) -> Result<Self, DescriptorError> {
        if !page_bytes.is_power_of_two() {
            return Err(DescriptorError::PageSize);
        }
        let version = match text(version) {
            Ok(text) => text,
            Err(error) => return Err(error),
        };
        let project = match text(project) {
            Ok(text) => text,
            Err(error) => return Err(error),
        };
        Ok(Self {
            magic: 0xabcd_5432,
            secure_version: 0,
            reserved1: [0; 2],
            version,
            project,
            time: *b"00:00:00\0\0\0\0\0\0\0\0",
            date: *b"2026-07-31\0\0\0\0\0\0",
            idf_version: *b"6.1\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0",
            elf_sha256: [0; 32],
            min_revision: 0,
            max_revision: u16::MAX,
            mmu_page_log2: page_bytes.ilog2() as u8,
            flash_mode: 0,
            reserved3: [0; 2],
            reserved2: [0; 18],
        })
    }
}

const fn text<const N: usize>(value: &str) -> Result<[u8; N], DescriptorError> {
    if value.len() >= N {
        return Err(DescriptorError::Text);
    }
    let mut output = [0; N];
    let mut index = 0;
    while index < value.len() {
        let byte = value.as_bytes()[index];
        if byte == 0 {
            return Err(DescriptorError::Text);
        }
        output[index] = byte;
        index += 1;
    }
    Ok(output)
}

// The loader/OTA ABI is fixed; a compiler layout change must fail compilation.
const _: () = {
    use core::mem::{offset_of, size_of};
    assert!(size_of::<AppDescriptor>() == 256);
    assert!(offset_of!(AppDescriptor, secure_version) == 4);
    assert!(offset_of!(AppDescriptor, elf_sha256) == 144);
    assert!(offset_of!(AppDescriptor, mmu_page_log2) == 180);
};

/// Place one descriptor in the bootstrap ELF under the name the host checks.
#[macro_export]
macro_rules! app_descriptor {
    ($version:expr, $project:expr, $page_bytes:expr) => {
        // SAFETY: the bootstrap linker owns this immutable metadata section;
        // exactly one invocation per image exports the loader ABI symbol.
        #[allow(
            unsafe_code,
            reason = "application descriptor export and linker placement"
        )]
        #[unsafe(export_name = "esp_app_desc")]
        #[unsafe(link_section = ".flash.appdesc")]
        #[used]
        pub static APP_DESCRIPTOR: $crate::AppDescriptor = const {
            match $crate::AppDescriptor::new($version, $project, $page_bytes) {
                Ok(descriptor) => descriptor,
                Err(_) => panic!("valid application descriptor metadata"),
            }
        };
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_is_terminated_and_never_truncated() {
        let descriptor = AppDescriptor::new("1", "radio", 65536).expect("descriptor");
        assert_eq!(&descriptor.project[..6], b"radio\0");
        assert!(descriptor.project[6..].iter().all(|byte| *byte == 0));
        assert!(matches!(
            AppDescriptor::new("1", &"a".repeat(32), 65536),
            Err(DescriptorError::Text)
        ));
        assert!(matches!(
            AppDescriptor::new("1\0hidden", "radio", 65536),
            Err(DescriptorError::Text)
        ));
    }

    #[test]
    fn page_size_must_have_an_exact_logarithm() {
        for invalid in [0, 3, 65535] {
            assert!(matches!(
                AppDescriptor::new("1", "radio", invalid),
                Err(DescriptorError::PageSize)
            ));
        }
        assert_eq!(
            AppDescriptor::new("1", "radio", 65536)
                .expect("descriptor")
                .mmu_page_log2,
            16
        );
    }
}
