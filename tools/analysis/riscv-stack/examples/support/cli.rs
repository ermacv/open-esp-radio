//! Examples keep their historical error status while clap renders diagnostics.
pub fn parse<T: clap::Parser>() -> T {
    T::try_parse().unwrap_or_else(|error| {
        let status = i32::from(error.use_stderr());
        let _ = error.print();
        std::process::exit(status);
    })
}
