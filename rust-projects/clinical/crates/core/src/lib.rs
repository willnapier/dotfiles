//! Shared types and helpers for the clinical workspace.
//!
//! Both the laptop notes toolchain (`clinical`) and the letter portal
//! server/client (`clinical-portal`) depend on this crate so that
//! identity.yaml schemas, client paths, and the data model live in
//! exactly one place.

pub mod client;
pub mod identity;
pub mod outcomes;
