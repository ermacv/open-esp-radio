//! Compile declarations shared with the verification image's root collector.

/// Declare a C entry point using a Rust signature followed by `=> expression;`
/// or an explicit function body. Conversions, owner setup and result projections
/// belong in the expression/body. Naked entries retain their explicit attribute.
/// Generic, async and conditional entry declarations are rejected: an async
/// production call must be driven by an explicit synchronous adapter.
#[proc_macro]
pub fn probe(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    match syn::parse::<oer_probe_codegen::Declaration>(input) {
        Ok(declaration) => declaration.expand().into(),
        Err(error) => error.into_compile_error().into(),
    }
}
