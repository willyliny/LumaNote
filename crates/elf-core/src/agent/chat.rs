//! The lightweight chat path: conversation without a task (handbook section 1).

use tokio_util::sync::CancellationToken;

use super::history::to_messages;
use super::turn::{TurnEnd, TurnParams};
use super::{ModelBinding, Orchestrator};
use crate::model::{MessagePart, ModelOrigin, ModelRequest, Role, StopReason};
use crate::schema::{ConversationId, ElfError, Result};
use crate::storage::StoredMessage;

/// The assistant's reply on the chat path.
#[derive(Debug, Clone, PartialEq)]
pub struct ChatReply {
    /// Conversation the exchange belongs to.
    pub conversation_id: ConversationId,
    /// The stored assistant message.
    pub message: StoredMessage,
    /// The reply text.
    pub text: String,
    /// Why the model stopped (e.g. `max_tokens` if the answer was cut off).
    pub finish: StopReason,
}

impl Orchestrator {
    /// Stores the user's message and streams a reply. No task is created, no tools are
    /// offered, and nothing here can change task state. Text deltas go to the event sink.
    /// If the stream fails after retries the user's message stays stored, so
    /// [`Orchestrator::chat_retry`] can ask again without duplicating it.
    pub async fn chat(
        &self,
        conversation: Option<&ConversationId>,
        user_text: &str,
        model: &ModelBinding,
        cancel: &CancellationToken,
    ) -> Result<ChatReply> {
        if user_text.trim().is_empty() {
            return Err(ElfError::invalid_input("message is empty"));
        }
        let now = self.clock.now_utc();
        let conv = self.store.with(|s| {
            let conv = match conversation {
                Some(id) => s.get_conversation(id)?.id,
                None => {
                    let title: String = user_text.trim().chars().take(60).collect();
                    s.create_conversation(&title, now)?.id
                }
            };
            s.append_message(
                &conv,
                None,
                Role::User,
                &[MessagePart::Text {
                    text: user_text.to_owned(),
                }],
                None,
                now,
            )?;
            Ok(conv)
        })?;
        self.chat_retry(&conv, model, cancel).await
    }

    /// Streams a reply to the newest user message of a conversation that has no answer yet.
    pub async fn chat_retry(
        &self,
        conversation: &ConversationId,
        model: &ModelBinding,
        cancel: &CancellationToken,
    ) -> Result<ChatReply> {
        let history = self.store.with(|s| s.list_messages(conversation))?;
        if history.last().map(|m| m.role) != Some(Role::User) {
            return Err(ElfError::invalid_input(
                "the conversation has no unanswered user message",
            ));
        }
        let origin = ModelOrigin::from_profile(&model.profile);
        let request = ModelRequest {
            output_budget: self.config.output_budget,
            idle_timeout: Some(self.config.model_idle_timeout),
            reasoning: self.config.reasoning.clone(),
            ..ModelRequest::new(
                model.profile.id.clone(),
                self.config.chat_instructions.clone(),
                to_messages(&history, &origin),
            )
        };
        let end = self
            .stream_turn(TurnParams {
                model,
                request,
                token: cancel,
                deadline: None,
                task_id: None,
                conversation_id: Some(conversation),
            })
            .await;
        match end {
            TurnEnd::Done(out) => {
                if out.stop == StopReason::ContextExceeded {
                    return Err(ElfError::ExternalFailure {
                        message: "the conversation no longer fits the model's context window"
                            .into(),
                        retryable: false,
                    });
                }
                if out.text.is_empty() {
                    return Err(ElfError::ExternalFailure {
                        message: "the model returned an empty reply".into(),
                        retryable: true,
                    });
                }
                let now = self.clock.now_utc();
                // text and provider state only: chat offers no tools, so a call the model
                // makes anyway (rejected by the adapter) is not part of the conversation
                let parts: Vec<MessagePart> = out
                    .parts
                    .iter()
                    .filter(|p| {
                        matches!(
                            p,
                            MessagePart::Text { .. } | MessagePart::ProviderState { .. }
                        )
                    })
                    .cloned()
                    .collect();
                let message = self.store.with(|s| {
                    s.append_message(conversation, None, Role::Assistant, &parts, None, now)
                })?;
                Ok(ChatReply {
                    conversation_id: conversation.clone(),
                    message,
                    text: out.text,
                    finish: out.stop,
                })
            }
            TurnEnd::Cancelled => Err(ElfError::Cancelled),
            TurnEnd::Deadline => Err(ElfError::Timeout {
                operation: "chat reply".into(),
            }),
            TurnEnd::Error { error, .. } => Err(error.to_elf_error()),
        }
    }
}
