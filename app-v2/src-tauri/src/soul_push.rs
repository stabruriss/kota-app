//! Host-side optional marks/Dream context. Storage remains in `crate::soul`;
//! classifier selection is separate from transport, delivery and accounting.
mod core;
pub(crate) mod input;
pub(crate) mod service;
pub(crate) mod settings;

#[cfg(test)]
mod tests;
