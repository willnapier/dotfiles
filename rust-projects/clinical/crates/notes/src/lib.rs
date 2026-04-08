// Re-export the shared core types so existing internal `use crate::identity::...`
// and `use crate::client::...` paths keep working with minimal churn.
pub use clinical_core::client;
pub use clinical_core::identity;

pub mod auth;
pub mod deidentify;
pub mod finalise;
pub mod letter;
pub mod markdown;
pub mod note;
pub mod populate;
pub mod prepare;
pub mod reidentify;
pub mod scaffold;
pub mod session;
