//! File listings of a directory: a scenario's attachments and the
//! canonical inventory of a directory's files.

use std::{fs, path::Path};

use crate::Result;
use crate::run::{Attachment, InventoryFile};
use oer_tool_base_durable::sha256_file;

pub fn collect_attachments(output: &Path, artifact_directory: &Path) -> Result<Vec<Attachment>> {
    let mut attachments = Vec::new();
    collect_attachments_below(output, Path::new(""), artifact_directory, &mut attachments)?;
    Ok(attachments)
}

fn collect_attachments_below(
    directory: &Path,
    relative: &Path,
    artifact_directory: &Path,
    attachments: &mut Vec<Attachment>,
) -> Result<()> {
    let mut entries = fs::read_dir(directory)?.collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let file_type = entry.file_type()?;
        let child_relative = relative.join(entry.file_name());
        if file_type.is_dir() {
            collect_attachments_below(
                &entry.path(),
                &child_relative,
                artifact_directory,
                attachments,
            )?;
        } else if file_type.is_file() {
            let metadata = entry.metadata()?;
            attachments.push(Attachment {
                path: artifact_directory.join(&child_relative),
                media_type: attachment_media_type(&child_relative).to_owned(),
                size_bytes: metadata.len(),
                sha256: sha256_file(&entry.path())?,
            });
        } else {
            return Err(format!(
                "HIL artifact is neither a regular file nor a directory: {}",
                entry.path().display()
            )
            .into());
        }
    }
    Ok(())
}

fn attachment_media_type(path: &Path) -> &'static str {
    match path.extension().and_then(|extension| extension.to_str()) {
        Some("json") => "application/json",
        Some("jsonl") => "application/x-ndjson",
        Some("pcap") | Some("pcapng") => "application/vnd.tcpdump.pcap",
        Some("html") => "text/html",
        Some("md") => "text/markdown",
        Some("log") | Some("txt") => "text/plain",
        _ => "application/octet-stream",
    }
}

/// Every regular file below `directory`, by relative path, with its size and
/// SHA-256, sorted by path. Anything else than a file or a directory is an
/// error.
pub fn inventory(directory: &Path) -> Result<Vec<InventoryFile>> {
    let mut files = Vec::new();
    inventory_below(directory, Path::new(""), &mut files)?;
    files.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(files)
}

fn inventory_below(
    directory: &Path,
    relative: &Path,
    files: &mut Vec<InventoryFile>,
) -> Result<()> {
    let mut entries = fs::read_dir(directory)?.collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let file_type = entry.file_type()?;
        let child_relative = relative.join(entry.file_name());
        if file_type.is_dir() {
            inventory_below(&entry.path(), &child_relative, files)?;
        } else if file_type.is_file() {
            files.push(InventoryFile {
                path: child_relative,
                size_bytes: entry.metadata()?.len(),
                sha256: sha256_file(&entry.path())?,
            });
        } else {
            return Err(format!(
                "neither a regular file nor a directory: {}",
                entry.path().display()
            )
            .into());
        }
    }
    Ok(())
}
