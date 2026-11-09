//! Revolut Business as MCP tools and prompts: the accounts, transactions, counterparties, cards
//! and payments of one business, through the Revolut Business API with the business's API key.

pub mod api;
pub mod config;
pub mod error;
pub mod prompts;
pub mod service;
pub mod signing;
pub mod tools;
pub mod view;

pub use service::{BankingService, from_env};

pub const VERSION: &str = match option_env!("APP_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};
