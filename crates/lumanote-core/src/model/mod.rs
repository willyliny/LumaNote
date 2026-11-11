//! Provider-neutral model contracts (handbook 7.1, 7.2), tool-call assembly, a scripted
//! mock adapter, the shared SSE parser and the two protocol adapters.

pub mod adapters;
pub mod assembler;
pub mod contracts;
pub mod mock;
#[cfg(feature = "http-client")]
pub mod reqwest_transport;
pub mod sse;
#[cfg(test)]
pub(crate) mod testkit;

pub use adapters::{
    AdapterDeps, AnthropicAdapter, AnthropicAuth, AnthropicConfig, ChatTokenParam,
    OpenAiChatAdapter, OpenAiChatConfig, PROTOCOL_ANTHROPIC, PROTOCOL_OPENAI_CHAT,
};
pub use assembler::ToolCallAssembler;
pub use contracts::*;
#[cfg(feature = "http-client")]
pub use reqwest_transport::{ReqwestTransport, ReqwestTransportConfig};
pub use sse::{SseError, SseEvent, SseParser};
