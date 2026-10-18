//! The repository's Cargo cache and model, read as text: `oer-tidy fetch`
//! downloads what every workspace's lock file names and the Cargo cache
//! lacks; the `workspaces` and `chips` commands print the repository model
//! of `oer-repo`.

pub mod fetch;

pub type Result<T> = std::result::Result<T, String>;
