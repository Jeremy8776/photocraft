//! The generative AI port for PhotoCraft.
//!
//! PhotoCraft does not know which service fills a selection. It knows this crate: a small
//! [`GenerationBackend`] trait that any service can implement (a local model, a user's own API key,
//! a generic HTTP endpoint), plus the pure work every backend would otherwise repeat:
//!
//! | Module | What it holds |
//! |---|---|
//! | [`image`] | [`Image`] (8-bit sRGB RGBA), [`Mask`] (coverage), [`IRect`], with enforced size caps |
//! | [`backend`] | [`GenerationBackend`], [`GenRequest`], [`GenResult`], [`Control`] (progress and cancel) |
//! | [`prep`] | [`plan_region`] (selection plus context, within the model's size budget) and [`build_request`] |
//! | [`apply`] | [`compose_patch`]: the result as a patch whose alpha is the selection |
//! | [`resize`] | alpha-aware resampling that averages when shrinking |
//! | [`mock`] | [`MockBackend`]: a scripted backend so everything above runs with no network |
//!
//! The crate has no dependencies, no I/O and no threads of its own, so it compiles for wasm and a
//! web build can still show the feature as unavailable with a clear message. It is deliberately
//! ignorant of documents, layers and colour management: the engine converts to and from sRGB
//! 8-bit [`Image`]s, so a backend behaves the same whatever the document's mode or depth.
//!
//! Nothing secret crosses this boundary. A backend is built already holding whatever credentials
//! it needs, and must not put them in an error message.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod apply;
pub mod backend;
pub mod error;
pub mod image;
pub mod mock;
pub mod prep;
pub mod resize;

pub use apply::compose_patch;
pub use backend::{Control, GenRequest, GenResult, GenerationBackend, NoControl, Task};
pub use error::{AiError, ErrorKind};
pub use image::{IRect, Image, Mask};
pub use mock::{MockBackend, MockBehavior};
pub use prep::{PrepLimits, RegionPlan, build_request, plan_region};
