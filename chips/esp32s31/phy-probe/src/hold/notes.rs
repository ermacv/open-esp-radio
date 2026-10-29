//! Where the hold observer's notes go.
use oer_diagnostics::Note;

// Diagnostic sinks and their host tests need no `Send` bound.
#[allow(async_fn_in_trait)]
pub trait Notes {
    async fn note(&mut self, note: &Note<'static>);
}
