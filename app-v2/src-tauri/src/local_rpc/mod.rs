//! Kota-owned Pi RPC processes. No terminal input, ANSI parsing or native-file scan.

mod ledger;
pub(crate) mod bundle;
pub(crate) mod credentials;
pub(crate) mod environment;
pub(crate) mod mcp;
pub(crate) mod error;
pub(crate) mod owned;
pub(crate) mod auth;
mod auth_protocol;
pub(crate) mod models;
pub(crate) mod jev;
pub(crate) mod jev_http;
pub(crate) mod magpie;
pub(crate) mod auto_route;
pub(crate) mod configuration;
pub(crate) mod query;
mod metadata_cache;
pub(crate) mod startup;
pub(crate) mod registry_types;
pub(crate) mod adapter;
pub(crate) mod commands;
pub(crate) mod launch;
mod bridge;
mod process;
mod protocol;
mod runtime;
pub(crate) mod shadow_clone;
mod workbench;
pub(crate) use workbench::{Cursor as WorkbenchCursor, View as WorkbenchView};

pub(crate) use protocol::{custom_receipt, CustomReceipt, DeliveryState, Injection, InjectionDetails, LocalNotice, LocalScope, OnlineState, Sink};
pub(crate) use runtime::{Config, LocalRuntime, Snapshot};
pub(crate) use launch::Discovery;
pub(crate) use bridge::{register_sink_factory, sink};

#[cfg(test)]
mod adapter_tests;
#[cfg(test)]
mod owned_tests;
#[cfg(test)]
pub(crate) mod auth_fixture;
#[cfg(test)]
mod auth_tests;
#[cfg(all(test, target_os = "macos"))]
mod auth_pi_tests;
#[cfg(test)]
mod model_tests;
#[cfg(test)]
mod auth_pin_tests;
#[cfg(test)]
mod command_route_tests;

#[cfg(test)]
mod lifecycle_tests;

#[cfg(test)]
mod pi_tests;
