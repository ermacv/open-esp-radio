//! The mask ROM no image may call into (`[rom].forbidden` of the chip's
//! `chip.toml`): no root's walk, which follows calls into the ROM, reaches a
//! function that overlaps a forbidden range. Like the bound, the check covers
//! what the walk resolved; a root's unresolved sites stay in its report.

use oer_analysis_riscv_stack::Bound;
use oer_chip_profile::RomRange;

use crate::stack::{Image, Result};

/// Fails naming, for each root of `roots` (a label and its bound) and each
/// range of `forbidden` it reaches, the first function it reaches there and
/// the path that reached it.
pub fn audit_forbidden<'a>(
    image: &Image,
    forbidden: &[RomRange],
    roots: impl IntoIterator<Item = (String, &'a Bound)>,
) -> Result<()> {
    let mut reached = Vec::new();
    for (label, bound) in roots {
        for range in forbidden {
            let Some(&function) = bound.reached.iter().find(|&&function| {
                let size = image
                    .analysis
                    .functions
                    .get(&function)
                    .map_or(1, |facts| facts.function.size.max(1));
                function < range.end && function.saturating_add(size) > range.start
            }) else {
                continue;
            };
            let mut path = vec![image.function_at(bound.root)];
            path.extend(
                bound
                    .witness(function)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|(callee, _)| image.function_at(callee)),
            );
            reached.push(format!(
                "{label} reaches forbidden ROM `{}` ({:#010x}..{:#010x}): {}",
                range.name,
                range.start,
                range.end,
                path.join(" -> ")
            ));
        }
    }
    if reached.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "the image calls into ROM no image may call:\n  {}",
            reached.join("\n  ")
        )
        .into())
    }
}
