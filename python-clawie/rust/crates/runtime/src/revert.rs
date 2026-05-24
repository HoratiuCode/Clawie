use crate::session::{ContentBlock, MessageRole, Session};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionCheckpoint {
    pub message_count: usize,
    pub last_user_message: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevertResult {
    pub reverted_session: Session,
    pub removed_message_count: usize,
}

#[must_use]
pub fn checkpoint(session: &Session) -> SessionCheckpoint {
    SessionCheckpoint {
        message_count: session.messages.len(),
        last_user_message: session
            .messages
            .iter()
            .rposition(|message| message.role == MessageRole::User),
    }
}

#[must_use]
pub fn revert_to_checkpoint(session: &Session, checkpoint: &SessionCheckpoint) -> RevertResult {
    let keep = checkpoint.message_count.min(session.messages.len());
    truncate_after(session, keep)
}

#[must_use]
pub fn revert_to_last_user_turn(session: &Session) -> RevertResult {
    let keep = session
        .messages
        .iter()
        .rposition(|message| message.role == MessageRole::User)
        .unwrap_or(session.messages.len());
    truncate_after(session, keep)
}

#[must_use]
pub fn truncate_after_tool_use(session: &Session, tool_use_id: &str) -> RevertResult {
    let keep = session
        .messages
        .iter()
        .position(|message| {
            message.blocks.iter().any(|block| match block {
                ContentBlock::ToolUse { id, .. } => id == tool_use_id,
                ContentBlock::ToolResult {
                    tool_use_id: id, ..
                } => id == tool_use_id,
                ContentBlock::Text { .. } => false,
            })
        })
        .unwrap_or(session.messages.len());
    truncate_after(session, keep)
}

fn truncate_after(session: &Session, keep: usize) -> RevertResult {
    let keep = keep.min(session.messages.len());
    RevertResult {
        reverted_session: Session {
            version: session.version,
            messages: session.messages[..keep].to_vec(),
        },
        removed_message_count: session.messages.len().saturating_sub(keep),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        checkpoint, revert_to_checkpoint, revert_to_last_user_turn, truncate_after_tool_use,
    };
    use crate::session::{ContentBlock, ConversationMessage, Session};

    #[test]
    fn reverts_to_checkpoint_message_count() {
        let mut session = Session::new();
        session.messages.push(ConversationMessage::user_text("one"));
        let checkpoint = checkpoint(&session);
        session.messages.push(ConversationMessage::user_text("two"));

        let result = revert_to_checkpoint(&session, &checkpoint);

        assert_eq!(result.removed_message_count, 1);
        assert_eq!(result.reverted_session.messages.len(), 1);
    }

    #[test]
    fn reverts_to_before_last_user_turn() {
        let mut session = Session::new();
        session.messages.push(ConversationMessage::user_text("one"));
        session
            .messages
            .push(ConversationMessage::assistant(vec![ContentBlock::Text {
                text: "done".to_string(),
            }]));
        session.messages.push(ConversationMessage::user_text("two"));

        let result = revert_to_last_user_turn(&session);

        assert_eq!(result.reverted_session.messages.len(), 2);
    }

    #[test]
    fn truncates_before_tool_use_message() {
        let mut session = Session::new();
        session.messages.push(ConversationMessage::user_text("one"));
        session.messages.push(ConversationMessage::assistant(vec![
            ContentBlock::ToolUse {
                id: "tool-1".to_string(),
                name: "bash".to_string(),
                input: "{}".to_string(),
            },
        ]));

        let result = truncate_after_tool_use(&session, "tool-1");

        assert_eq!(result.reverted_session.messages.len(), 1);
    }
}
