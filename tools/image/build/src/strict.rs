//! Validate Cargo's cached runtime link against the linker's actual inputs.

use crate::Result;
use oer_image_checks::linker::hot::FRAGMENT as HOT_FRAGMENT;
use oer_image_checks::linker::strict::{FRAGMENT, PLACEMENT, Placement, SPEC_ENV, SPEC_FILE, Spec};
use std::{
    path::{Path, PathBuf},
    process::Command,
};

pub(crate) fn configure(
    command: &mut Command,
    spec: &Spec,
    output: &Path,
    interrupts: bool,
) -> Result<PathBuf> {
    let analysis = Spec {
        root: std::fs::canonicalize(&spec.root)?,
        chip: spec.chip.clone(),
        stack_policy: std::fs::canonicalize(spec.root.join(&spec.stack_policy))?,
        diagnostic: spec.diagnostic,
        task_stack_proven: spec.task_stack_proven,
        harts: spec.harts.clone(),
        hot_code: spec
            .hot_code
            .as_ref()
            .map(|path| {
                std::fs::canonicalize(path)
                    .map_err(|error| format!("hot-code list {}: {error}", path.display()))
            })
            .transpose()?,
    };
    let path = std::path::absolute(output.join(SPEC_FILE))?;
    std::fs::write(&path, toml::to_string(&analysis)?)?;
    // Only an image without an interrupt contract has known empty results
    // before Cargo runs. A reused strict link must keep its original record.
    if !interrupts {
        std::fs::write(output.join(FRAGMENT), "")?;
        std::fs::write(output.join(HOT_FRAGMENT), "")?;
        std::fs::write(
            output.join(PLACEMENT),
            toml::to_string(&Placement::default())?,
        )?;
    }
    command.env(SPEC_ENV, &path);
    Ok(path)
}

pub(crate) fn build_with_retry(
    mut build: impl FnMut(bool) -> Result<()>,
    mut matches: impl FnMut() -> Result<bool>,
) -> Result<()> {
    build(false)?;
    if matches()? {
        return Ok(());
    }
    eprintln!("runtime link fingerprint differs; rebuilding the runtime package once");
    build(true)?;
    if !matches()? {
        return Err(
            "runtime link fingerprint still differs after rebuilding the runtime package".into(),
        );
    }
    Ok(())
}

pub(crate) fn restore_if_matching(
    output: &Path,
    published: &Path,
    elf: &Path,
    specification: &Path,
    linker: &Path,
) -> Result<bool> {
    // A linker invocation writes into staging. Cargo Fresh leaves it absent,
    // so validate the published record and restore it with its exact fragment.
    let source = if output.join(PLACEMENT).is_file() {
        output
    } else {
        published
    };
    let placement = match Placement::read(source) {
        Ok(placement) => placement,
        Err(error) => {
            eprintln!("runtime placement record cannot validate the link: {error}");
            return Ok(false);
        }
    };
    let Some(fingerprint) = &placement.fingerprint else {
        return Ok(false);
    };
    if !fingerprint.matches(elf, specification, linker)? {
        return Ok(false);
    }
    let fragment = match std::fs::read(source.join(FRAGMENT)) {
        Ok(fragment) => fragment,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    if fragment != placement.fragment().as_bytes() {
        return Ok(false);
    }
    let hot = match std::fs::read(source.join(HOT_FRAGMENT)) {
        Ok(fragment) => fragment,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    if hot != placement.hot_fragment().as_bytes() {
        return Ok(false);
    }
    if source != output {
        for file in [FRAGMENT, HOT_FRAGMENT, PLACEMENT] {
            std::fs::copy(source.join(file), output.join(file))?;
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configuring_interrupt_analysis_does_not_erase_linker_results() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        std::fs::write(root.join("stack.toml"), "policy").unwrap();
        let spec = Spec {
            root: root.to_owned(),
            chip: "test".into(),
            stack_policy: "stack.toml".into(),
            diagnostic: false,
            task_stack_proven: false,
            harts: Vec::new(),
            hot_code: None,
        };
        let mut command = Command::new("not-executed");
        configure(&mut command, &spec, root, true).unwrap();
        assert!(!root.join(FRAGMENT).exists());
        assert!(!root.join(PLACEMENT).exists());
        for file in [FRAGMENT, PLACEMENT] {
            std::fs::write(root.join(file), "preserved").unwrap();
        }
        configure(&mut command, &spec, root, true).unwrap();
        for file in [FRAGMENT, PLACEMENT] {
            assert_eq!(
                std::fs::read_to_string(root.join(file)).unwrap(),
                "preserved"
            );
        }
        configure(&mut command, &spec, root, false).unwrap();
        assert_eq!(std::fs::read(root.join(FRAGMENT)).unwrap(), b"");
        assert_eq!(std::fs::read(root.join(HOT_FRAGMENT)).unwrap(), b"");
        assert!(Placement::read(root).unwrap().functions.is_empty());
    }

    #[test]
    fn reused_elf_keeps_its_nonempty_selection_and_fragment_in_the_new_bundle() {
        use oer_image_checks::linker::strict::{Fingerprint, PlacedFunction};
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let output = root.join("staging");
        let published = root.join("published");
        for path in [&output, &published] {
            std::fs::create_dir(path).unwrap();
        }
        let elf = root.join("runtime.elf");
        let linker = root.join("linker");
        let specification = output.join(SPEC_FILE);
        for path in [&elf, &linker, &specification] {
            std::fs::write(path, b"unchanged").unwrap();
        }
        let digest = oer_os_durable::sha256_bytes(b"unchanged");
        let placement = Placement {
            irq_stacks: Vec::new(),
            task_stacks: Vec::new(),
            functions: vec![PlacedFunction {
                name: "strict".into(),
                symbol: "strict".into(),
                file: "input.o".into(),
                section: ".assembly".into(),
                section_index: 1,
                size: 4,
                section_bytes: 8,
                other_symbols: vec!["neighbour".into()],
            }],
            hot: vec![PlacedFunction {
                name: "hot".into(),
                symbol: "hot".into(),
                file: "input.o".into(),
                section: ".text.hot".into(),
                section_index: 2,
                size: 16,
                section_bytes: 16,
                other_symbols: Vec::new(),
            }],
            hot_skipped: Vec::new(),
            fingerprint: Some(Fingerprint {
                elf_sha256: digest.clone(),
                linker_sha256: digest.clone(),
                inputs: [(specification.clone(), digest)].into(),
            }),
        };
        let text = toml::to_string(&placement).unwrap();
        std::fs::write(published.join(PLACEMENT), &text).unwrap();
        std::fs::write(published.join(FRAGMENT), placement.fragment()).unwrap();
        // A hot fragment that differs from the record invalidates the link.
        std::fs::write(published.join(HOT_FRAGMENT), "").unwrap();
        assert!(!restore_if_matching(&output, &published, &elf, &specification, &linker).unwrap());
        std::fs::write(published.join(HOT_FRAGMENT), placement.hot_fragment()).unwrap();
        assert!(restore_if_matching(&output, &published, &elf, &specification, &linker).unwrap());
        assert_eq!(
            std::fs::read_to_string(output.join(HOT_FRAGMENT)).unwrap(),
            "\"input.o\"(\".text.hot\");\n"
        );
        assert_eq!(
            std::fs::read_to_string(output.join(PLACEMENT)).unwrap(),
            text
        );
        assert_eq!(
            std::fs::read_to_string(output.join(FRAGMENT)).unwrap(),
            placement.fragment()
        );
        assert_eq!(
            Placement::read(&output).unwrap().functions[0].other_symbols,
            ["neighbour"]
        );
        std::fs::write(&elf, b"another build").unwrap();
        assert!(!restore_if_matching(&output, &published, &elf, &specification, &linker).unwrap());
    }

    #[test]
    fn fresh_fingerprint_never_cleans_and_stale_fingerprint_rebuilds_exactly_once() {
        for checks in [vec![true], vec![false, true], vec![false, false]] {
            let mut builds = Vec::new();
            let mut checks_iter = checks.iter();
            let result = build_with_retry(
                |invalidate| {
                    builds.push(invalidate);
                    Ok(())
                },
                || {
                    Ok(*checks_iter
                        .next()
                        .expect("unexpected extra fingerprint check"))
                },
            );
            assert_eq!(
                builds,
                if checks.len() == 1 {
                    vec![false]
                } else {
                    vec![false, true]
                }
            );
            assert_eq!(result.is_ok(), *checks.last().unwrap());
            assert_eq!(checks_iter.next(), None);
            if let Err(error) = result {
                assert!(error.to_string().contains("still differs"));
            }
        }
    }

    #[test]
    fn a_failed_cargo_build_is_not_retried() {
        let mut builds = Vec::new();
        assert!(
            build_with_retry(
                |invalidate| {
                    builds.push(invalidate);
                    Err("cargo failed".into())
                },
                || panic!("a failed build must not be validated")
            )
            .is_err()
        );
        assert_eq!(builds, [false]);
    }
}
