//! Where the hold observer's notes go.
use oer_diagnostics::Note;

// Used only inside this image and its host test, whose futures need no
// `Send` bound.
#[allow(async_fn_in_trait)]
pub trait Notes {
    async fn note(&mut self, note: &Note<'static>);
}
