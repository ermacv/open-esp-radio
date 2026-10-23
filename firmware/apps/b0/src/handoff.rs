//! Failed installation answers its caller while retaining the affine owner.
pub async fn park_failure<T>(retained: T, answer: impl FnOnce()) -> ! {
    let _retained = retained;
    answer();
    match core::future::pending::<core::convert::Infallible>().await {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::{
        cell::Cell,
        future::Future,
        pin::pin,
        task::{Context, Waker},
    };
    #[test]
    fn failure_answers_before_parking_and_retains_the_owner() {
        struct Owner<'a>(&'a Cell<bool>);
        impl Drop for Owner<'_> {
            fn drop(&mut self) {
                self.0.set(true);
            }
        }
        let dropped = Cell::new(false);
        let answered = Cell::new(false);
        let mut failed = pin!(park_failure(Owner(&dropped), || answered.set(true)));
        assert!(
            failed
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop()))
                .is_pending()
        );
        assert!(answered.get());
        assert!(!dropped.get());
    }
}
