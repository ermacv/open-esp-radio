//! The pin-specific bridge: session, codegen inventory and macro call sites.
extern crate rustc_driver;
extern crate rustc_interface;
extern crate rustc_middle;
extern crate rustc_span;

use std::{collections::HashSet, process::ExitCode};

use rustc_driver::{Callbacks, Compilation};
use rustc_middle::{mono::MonoItem, ty::TyCtxt};
use rustc_public::{mir::mono::Instance, ty::Span};

pub fn run(
    arguments: &[String],
    collect: impl FnMut(&[Instance], &dyn Fn(Span) -> String) + Send,
) -> ExitCode {
    rustc_driver::catch_with_exit_code(|| {
        rustc_driver::run_compiler(arguments, &mut Reporter(collect));
    })
}

struct Reporter<F>(F);

impl<F: FnMut(&[Instance], &dyn Fn(Span) -> String)> Callbacks for Reporter<F> {
    fn after_analysis<'tcx>(
        &mut self,
        _: &rustc_interface::interface::Compiler,
        tcx: TyCtxt<'tcx>,
    ) -> Compilation {
        rustc_public::rustc_internal::run(tcx, || {
            let mut seen = HashSet::new();
            // The pinned public API cannot enumerate codegen instances.
            let instances: Vec<Instance> = tcx
                .collect_and_partition_mono_items(())
                .codegen_units
                .iter()
                .flat_map(|unit| unit.items().keys())
                .filter_map(|item| match *item {
                    MonoItem::Fn(instance) if seen.insert(instance) => {
                        Some(rustc_public::rustc_internal::stable(instance))
                    }
                    _ => None,
                })
                .collect();
            (self.0)(&instances, &|span| {
                // The pinned public Span has no macro expansion/callsite API.
                let span: rustc_span::Span = rustc_public::rustc_internal::internal(tcx, span);
                let span: Span = rustc_public::rustc_internal::stable(span.source_callsite());
                span.diagnostic()
            });
        })
        .expect("the compiler callback owns the public MIR context");
        Compilation::Continue
    }
}
