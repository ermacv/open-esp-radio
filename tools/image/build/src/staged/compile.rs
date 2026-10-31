//! Compile, snapshot and check the runtime, then compile and snapshot the
//! embedding bootstrap. Finalization always audits that ELF before encoding.

use super::{CompileInput, Compiled};
use crate::encode::stage_two::pack_runtime;
use crate::{Result, strict};
use oer_image_bundle::{ImageBundle, files};

pub(crate) fn cargo(input: &CompileInput<'_>, bundle: &mut ImageBundle) -> Result<Compiled> {
    let CompileInput {
        spec,
        profile,
        instrumentation,
        policy,
        task_stack_proven,
        critical_section_hart,
        harts,
        panics,
        output,
        log,
    } = *input;
    let root = spec.root.as_path();
    let target = profile.facts.rust_target.as_str();
    let bootstrap_package = profile.bootstrap_package();
    // The runtime and the bootstrap compile with the image's instrumentation,
    // in its own cache.
    let cache = crate::compile_cache(root, instrumentation);
    let mut values = crate::large_values::Report::new(policy.max_move_bytes);
    let mut sizes = crate::sizes::Summary::new(spec, profile)?;
    crate::test_support::refuse(root, &spec.application.package, target)?;

    // Every image uplifts into the same `<target>/firmware` of the
    // checkout's cache: each build holds it until its ELF and dependency information
    // are copied out.
    let release = cache.join(target).join(crate::FIRMWARE_PROFILE);
    let runtime_elf = {
        let _cache = crate::compiler::lock_compile_cache(&cache)?;
        let mut runtime = crate::runtime_command(spec, profile, "build", policy, instrumentation)?;
        runtime.env("CARGO_TARGET_DIR", &cache);
        sizes.flags("runtime", &runtime, root)?;
        let strict = profile.facts.memory.is_some() && profile.facts.interrupts.is_some();
        let analysis = oer_image_checks::linker::strict::Spec {
            root: spec.root.clone(),
            chip: spec.chip.clone(),
            stack_policy: spec.stack_policy.clone(),
            diagnostic: spec.checks.as_ref().and_then(|checks| checks.interrupts)
                != Some(oer_image_checks::interrupts::Required::Product),
            task_stack_proven,
            harts: harts.to_vec(),
        };
        let specification = strict::configure(&mut runtime, &analysis, output, strict)?;
        crate::cargo::ensure_fetched(root)?;
        let mut build = |invalidate| -> Result<()> {
            if invalidate {
                let mut clean = crate::cargo::command();
                clean
                    .current_dir(root)
                    .args([
                        "clean",
                        "--locked",
                        "--profile",
                        crate::FIRMWARE_PROFILE,
                        "--target",
                        target,
                        "-p",
                        &spec.application.package,
                    ])
                    .env("CARGO_TARGET_DIR", &cache);
                log.run(
                    &mut clean,
                    "invalidate the runtime link after a fingerprint mismatch",
                )?;
            }
            let _slot = crate::exclusion::slot(&crate::host_build_root()?.join("tokens"))?;
            values = crate::large_values::build(
                &mut runtime,
                log,
                root,
                &cache,
                target,
                crate::FIRMWARE_PROFILE,
                "runtime",
                output,
            )?;
            Ok(())
        };
        if strict {
            strict::build_with_retry(&mut build, || {
                strict::restore_if_matching(
                    output,
                    &spec.output,
                    &release.join(&spec.application.binary),
                    &specification,
                    &crate::compiler::linker_target(root).join("debug/oer-image-linker"),
                )
            })?;
        } else {
            build(false)?;
        }
        let elf = crate::snapshot(
            &release.join(&spec.application.binary),
            output,
            files::RUNTIME_ELF,
        )?;
        {
            use oer_image_checks::linker::strict::{FRAGMENT, PLACEMENT, SPEC_FILE};
            for file in [FRAGMENT, PLACEMENT, SPEC_FILE] {
                if !output.join(file).is_file() {
                    return Err(format!("the runtime linker did not produce {file}").into());
                }
                bundle.reports.push(file.to_owned());
            }
        }
        elf
    };
    let runtime_bin = output.join(files::RUNTIME_BIN);
    let mut objcopy = oer_os_toolchain::command(oer_os_toolchain::Tool::LlvmObjcopy)?;
    objcopy
        .args(["-O", "binary"])
        .arg(&runtime_elf)
        .arg(&runtime_bin);
    log.run(&mut objcopy, "flatten the stage-two runtime")?;
    let staged = profile
        .facts
        .staged
        .as_ref()
        .ok_or("a staged chip's profile names its [staged] contract")?;
    let crc = pack_runtime(&staged.stage_two, &runtime_bin)
        .map_err(|error| log.failed("runtime packing", error))?;
    eprintln!("runtime_crc32={crc:08x}");
    {
        let checks = spec.checks.as_ref();
        let prerequisites = crate::Gates::default();
        let gates = match checks {
            Some(checks) => checks,
            None => &prerequisites,
        };
        let outcome = gates
            .runtime(&crate::CheckInput {
                root,
                profile,
                policy,
                elf: &runtime_elf,
                flat: Some(&runtime_bin),
                output,
                task_stack_proven,
                critical_section_hart,
                stage_two_elf: None,
                panics: Some(panics),
            })
            .map_err(|error| log.failed("runtime checks", error))?;
        if let Some(measured) = &outcome.sizes {
            sizes.elfs.insert("runtime".into(), measured.clone());
        }
        crate::apply_outcome(
            gates,
            crate::CheckedElf::Runtime,
            outcome,
            bundle,
            task_stack_proven,
        )?;
    }

    // The bootstrap embeds the runtime with `include_bytes!`; a stable path
    // that keeps its timestamp while its bytes are unchanged leaves the
    // bootstrap fresh.
    let bootstrap_cache = crate::compiler::lock_compile_cache(&cache)?;
    let embedded = std::path::absolute(cache.join("stage-two-runtime.bin"))?;
    crate::replace_if_changed(&runtime_bin, &embedded)?;
    let mut bootstrap = crate::cargo::command();
    bootstrap
        .current_dir(root)
        .args(["build", "--locked", "-p", &bootstrap_package])
        .args(["--profile", crate::FIRMWARE_PROFILE, "--target", target])
        .env("CARGO_TARGET_DIR", &cache)
        .env("CARGO_INCREMENTAL", "0")
        .env("PSRAM_RUNTIME_BIN", &embedded);
    for (name, _) in &spec.runtime_environment {
        bootstrap.env_remove(name);
    }
    crate::zeroed_inputs(&mut bootstrap, profile);
    crate::compiler::configure(
        &mut bootstrap,
        &crate::compiler::of_policy(
            policy,
            instrumentation,
            root,
            &crate::linker_cache(root),
            target,
        ),
    )?;
    sizes.flags("bootstrap", &bootstrap, root)?;
    values.append(crate::large_values::build(
        &mut bootstrap,
        log,
        root,
        &cache,
        target,
        crate::FIRMWARE_PROFILE,
        "bootstrap",
        output,
    )?);
    let bootstrap_elf = crate::snapshot(
        &release.join(&bootstrap_package),
        output,
        files::BOOTSTRAP_ELF,
    )?;
    drop(bootstrap_cache);
    Ok(Compiled {
        runtime_elf,
        runtime_crc32: crc,
        bootstrap_elf,
        values,
        sizes,
    })
}
