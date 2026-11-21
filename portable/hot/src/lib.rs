//! `#[hot(N)]`: how hot a function is, said where it is written.
//!
//! Level 1 is the best gain a byte; a higher level is worth less or costs
//! more. The attribute says nothing of memory: an image names the highest
//! level it takes (`[package.metadata.open-radio] hot`), and the link places
//! the functions of those levels in SRAM and leaves the others with ordinary
//! code (`docs/memory.md`, Code placement).
//!
//! On a bare-metal target (`target_os = "none"`) the attribute writes
//! `#[inline(never)]`, so the function keeps a place of its own, and
//! `#[unsafe(link_section = ".hot.<N>.text.<module path>::<name>")]`, a
//! section of its own a function, so the linker still drops what nothing
//! calls. On the host it writes nothing.
//!
//! The section attribute is unsafe in Rust 2024; this crate is its home
//! (`docs/unsafe.md`). Its contract: it takes functions only, and writes
//! section names of the one family `.hot.<N>.text.*`, which the platform's
//! runtime script maps to executable memory, chosen or not. A wrong level
//! costs SRAM, never soundness. The using crate keeps
//! `#![forbid(unsafe_code)]`: the attribute is the expansion's, not its own.

#![forbid(unsafe_code)]
#![cfg_attr(
    test,
    allow(
        clippy::disallowed_methods,
        clippy::disallowed_macros,
        clippy::unreachable,
        clippy::arithmetic_side_effects
    )
)]

extern crate proc_macro;

use proc_macro::{Delimiter, Group, Ident, Literal, Punct, Spacing, Span, TokenStream, TokenTree};

mod plan;

use plan::{Delim, Shape, Token};

/// Mark a function hot at level `N`, from 1 to 3.
///
/// ```ignore
/// #[oer_portable_hot::hot(1)]
/// fn per_frame_step() {}
/// ```
#[proc_macro_attribute]
pub fn hot(attribute: TokenStream, item: TokenStream) -> TokenStream {
    let written = match plan::plan(&shapes(attribute), &shapes(item.clone())) {
        Ok(placement) => placement.attributes(),
        Err(refused) => refused.tokens(),
    };
    let mut expanded = stream(&written);
    expanded.extend(item);
    expanded
}

/// The top-level shape of each token tree: what the plan reads.
fn shapes(stream: TokenStream) -> Vec<Shape> {
    stream
        .into_iter()
        .map(|tree| match tree {
            TokenTree::Ident(ident) => Shape::Ident(ident.to_string()),
            TokenTree::Punct(punct) => Shape::Punct(punct.as_char()),
            TokenTree::Literal(literal) => Shape::Literal(literal.to_string()),
            TokenTree::Group(group) => Shape::Group(match group.delimiter() {
                Delimiter::Parenthesis => Delim::Parenthesis,
                Delimiter::Brace => Delim::Brace,
                Delimiter::Bracket => Delim::Bracket,
                Delimiter::None => Delim::None,
            }),
        })
        .collect()
}

/// The compiler's tokens of what the plan writes.
fn stream(tokens: &[Token]) -> TokenStream {
    tokens
        .iter()
        .map(|token| match token {
            Token::Ident(text) => TokenTree::Ident(Ident::new(text, Span::call_site())),
            Token::Punct(character, joint) => TokenTree::Punct(Punct::new(
                *character,
                match joint {
                    plan::Joint::Joint => Spacing::Joint,
                    plan::Joint::Alone => Spacing::Alone,
                },
            )),
            Token::Str(text) => TokenTree::Literal(Literal::string(text)),
            Token::Group(delim, inner) => TokenTree::Group(Group::new(
                match delim {
                    Delim::Parenthesis => Delimiter::Parenthesis,
                    Delim::Brace => Delimiter::Brace,
                    Delim::Bracket => Delimiter::Bracket,
                    Delim::None => Delimiter::None,
                },
                stream(inner),
            )),
        })
        .collect()
}
