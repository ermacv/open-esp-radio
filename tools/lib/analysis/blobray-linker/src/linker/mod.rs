//! External ELF capability dispatcher. Application owns selection and publication.
use super::*;
use blobray_application::linking::{
    LinkInput, LinkInvocation, LinkOutput, LinkOutputSink, LinkWorkspace, LinkerHost,
    UnresolvedSymbols,
};
use std::{
    ffi::OsString,
    io::Read,
    os::{
        fd::AsRawFd,
        unix::{
            ffi::OsStringExt,
            process::{CommandExt, ExitStatusExt},
        },
    },
};
mod gnu_ld;
mod lld;
mod process;
mod records;
use process::*;
pub struct ElfLinker;

fn blocked(message: impl Into<String>) -> Error {
    Error::new(ErrorCode::LinkBlocked, message)
}
impl LinkerHost for ElfLinker {
    fn identify(
        &self,
        path: &Path,
        workspace: &LinkWorkspace,
        control: &mut dyn RunControl,
    ) -> Result<LinkerIdentity> {
        let path = path.canonicalize().map_err(io)?;
        let before = digest(&path, control)?;
        let mut child = ChildOwner(command(&path).arg("--version").spawn().map_err(io)?);
        let stdout = child.0.stdout.take().unwrap();
        let stderr = child.0.stderr.take().unwrap();
        let mut version = Version {
            stdout: Vec::new(),
            total: 0,
        };
        drain(
            &mut child,
            &[
                (stdout.as_raw_fd(), LinkOutput::Elf),
                (stderr.as_raw_fd(), LinkOutput::Stderr),
            ],
            &mut version,
            control,
        )?;
        let text = std::str::from_utf8(&version.stdout)
            .map_err(|_| Error::new(ErrorCode::Incompatible, "linker version is not UTF-8"))?;
        let version = text.lines().next().unwrap_or("").trim();
        let implementation = if version.starts_with("LLD ") {
            "lld-elf"
        } else if version.starts_with("GNU ld ") {
            "gnu-ld-elf"
        } else {
            return Err(Error::new(
                ErrorCode::Incompatible,
                "unsupported ELF linker family",
            ));
        };
        let identity = LinkerIdentity {
            implementation: implementation.into(),
            version: version.into(),
            executable: before,
        };
        workspace.probe(self, &path, &identity, control)?;
        if identity.executable != digest(&path, control)? {
            return Err(Error::new(
                ErrorCode::SourceChanged,
                "linker changed during identification",
            ));
        }
        Ok(identity)
    }
    fn link(
        &self,
        request: &LinkInvocation<'_>,
        sink: &mut dyn LinkOutputSink,
        control: &mut dyn RunControl,
    ) -> Result<()> {
        if digest(request.executable, control)? != request.identity.executable {
            return Err(Error::new(
                ErrorCode::SourceChanged,
                "linker changed before launch",
            ));
        }
        match request.identity.implementation.as_str() {
            "lld-elf" => lld::link(request, sink, control)?,
            "gnu-ld-elf" => gnu_ld::link(request, sink, control)?,
            _ => {
                return Err(Error::new(
                    ErrorCode::Incompatible,
                    "unsupported ELF linker adapter",
                ));
            }
        }
        if digest(request.executable, control)? != request.identity.executable {
            return Err(Error::new(
                ErrorCode::SourceChanged,
                "linker changed during execution",
            ));
        }
        Ok(())
    }
}
struct Version {
    stdout: Vec<u8>,
    total: usize,
}
impl LinkOutputSink for Version {
    fn write(
        &mut self,
        channel: LinkOutput,
        bytes: &[u8],
        control: &mut dyn RunControl,
    ) -> Result<()> {
        control.bytes(bytes.len())?;
        if self.total + bytes.len() > 4096 {
            return Err(Error::new(
                ErrorCode::Incompatible,
                "linker version exceeds 4 KiB",
            ));
        }
        self.total += bytes.len();
        if channel == LinkOutput::Elf {
            self.stdout.extend_from_slice(bytes);
        }
        Ok(())
    }
    fn observe(&mut self, _: LinkObservation, _: &mut dyn RunControl) -> Result<()> {
        Ok(())
    }
    fn elf(&mut self, _: Vec<u8>) -> Result<()> {
        Err(blocked("unexpected file during identification"))
    }
}
fn semantic_arguments(request: &LinkInvocation<'_>, command: &mut Command) -> Result<()> {
    let mut script = layout_script(request.layout);
    for (name, address) in &request.definitions {
        use std::fmt::Write;
        writeln!(&mut script, "{name} = {address:#x};").unwrap();
    }
    request
        .workspace
        .materialize("layout.ld", script.as_bytes())?;
    command
        .current_dir(request.workspace.directory())
        .args([
            "-m",
            "elf32lriscv",
            "--gc-sections",
            "--no-relax",
            "--emit-relocs",
            "--no-demangle",
            "--build-id=none",
            "-T",
            "layout.ld",
        ])
        .args(match request.unresolved {
            UnresolvedSymbols::Error => &["--error-unresolved-symbols", "--no-undefined"][..],
            UnresolvedSymbols::Report => &["--unresolved-symbols=ignore-all"][..],
        })
        .arg("--entry")
        .arg(OsString::from_vec(request.entry.into()))
        .arg("--undefined")
        .arg(OsString::from_vec(request.entry.into()));
    for root in &request.roots {
        command
            .arg("--undefined")
            .arg(OsString::from_vec(root.clone()));
    }
    command.args(&request.forced);
    for input in &request.inputs {
        match input {
            LinkInput::Object(path) => {
                command.arg(path);
            }
            LinkInput::Archive(paths) => {
                command.arg("--start-lib").args(paths).arg("--end-lib");
            }
        }
    }
    Ok(())
}
fn inherit(command: &mut Command, descriptors: Vec<i32>, maximum: Option<u64>) {
    // SAFETY: only fcntl/setrlimit run after fork; descriptors are retained until spawn.
    unsafe {
        command.pre_exec(move || {
            for fd in &descriptors {
                if libc::fcntl(*fd, libc::F_SETFD, 0) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
            }
            if let Some(maximum) = maximum {
                let limit = libc::rlimit {
                    rlim_cur: maximum,
                    rlim_max: maximum,
                };
                if libc::setrlimit(libc::RLIMIT_FSIZE, &limit) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
            }
            Ok(())
        });
    }
}
fn layout_script(layout: ImageLayout) -> String {
    format!(
        "MEMORY {{ CODE (rx) : ORIGIN = {:#x}, LENGTH = {:#x}\n DATA (rw) : ORIGIN = {:#x}, LENGTH = {:#x} }}\nPHDRS {{ code PT_LOAD FLAGS(5); data PT_LOAD FLAGS(6); }}\nSECTIONS {{ .text : {{ INPUT_SECTION_FLAGS (SHF_ALLOC & SHF_EXECINSTR) *(*) }} > CODE :code\n .rodata : {{ *(.rodata .rodata.* .srodata .srodata.*) }} > CODE :code\n .eh_frame : {{ *(.eh_frame) }} > CODE :code\n .data : {{ *(.data .data.* .sdata .sdata.*) }} > DATA :data\n PROVIDE(__global_pointer$ = ADDR(.data) + 0x800);\n .bss (NOLOAD) : {{ *(.bss .bss.* .sbss .sbss.*) *(COMMON) }} > DATA :data\n }}\n",
        layout.code.start, layout.code.length, layout.data.start, layout.data.length
    )
}

#[cfg(test)]
mod tests;
