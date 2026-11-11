//! The two protocol adapters of the first version (handbook 7.1):
//! Anthropic Messages and OpenAI Chat Completions (also used for "OpenAI-compatible"
//! cloud services and local servers). They are free of any concrete HTTP client: all I/O
//! goes through [`crate::ports::HttpTransport`].

mod anthropic;
mod common;
mod openai_chat;

pub use anthropic::{AnthropicAdapter, AnthropicAuth, AnthropicConfig};
pub use common::{AdapterDeps, PROTOCOL_ANTHROPIC, PROTOCOL_OPENAI_CHAT};
pub use openai_chat::{ChatTokenParam, OpenAiChatAdapter, OpenAiChatConfig};

#[cfg(test)]
mod tests;
