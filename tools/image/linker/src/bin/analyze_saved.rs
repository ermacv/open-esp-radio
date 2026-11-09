//! Re-run the image linker's first-pass analysis on a saved runtime ELF,
//! with the specification the image build wrote (`image-linker.toml`). No
//! link and no image build run: `oer-image-analyze-saved SPEC ELF`.

#![forbid(unsafe_code)]

fn main() -> std::process::ExitCode {
    let arguments: Vec<std::path::PathBuf> = std::env::args_os().skip(1).map(Into::into).collect();
    let [spec, elf] = arguments.as_slice() else {
        eprintln!("usage: oer-image-analyze-saved SPEC ELF");
        return std::process::ExitCode::from(2);
    };
    match oer_tools_image_checks::linker::analyze_saved(spec, elf) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("oer-image-analyze-saved: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}
