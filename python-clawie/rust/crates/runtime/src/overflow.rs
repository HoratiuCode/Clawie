use crate::compact::{estimate_session_tokens, should_compact, CompactionConfig};
use crate::session::Session;

const DEFAULT_COMPACTION_BUFFER: usize = 20_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextWindow {
    pub context_tokens: usize,
    pub input_tokens: Option<usize>,
    pub output_token_max: usize,
    pub reserved_tokens: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OverflowStatus {
    pub estimated_tokens: usize,
    pub usable_tokens: usize,
    pub is_overflow: bool,
    pub should_compact: bool,
}

impl ContextWindow {
    #[must_use]
    pub fn usable_tokens(self) -> usize {
        if self.context_tokens == 0 {
            return 0;
        }

        let reserved = self
            .reserved_tokens
            .unwrap_or_else(|| DEFAULT_COMPACTION_BUFFER.min(self.output_token_max));
        self.input_tokens
            .unwrap_or(self.context_tokens.saturating_sub(self.output_token_max))
            .saturating_sub(reserved)
    }
}

#[must_use]
pub fn status(
    session: &Session,
    window: ContextWindow,
    config: CompactionConfig,
) -> OverflowStatus {
    let estimated_tokens = estimate_session_tokens(session);
    let usable_tokens = window.usable_tokens();
    OverflowStatus {
        estimated_tokens,
        usable_tokens,
        is_overflow: usable_tokens > 0 && estimated_tokens >= usable_tokens,
        should_compact: should_compact(session, config),
    }
}

#[cfg(test)]
mod tests {
    use super::{status, ContextWindow};
    use crate::compact::CompactionConfig;
    use crate::session::{ConversationMessage, Session};

    #[test]
    fn computes_usable_tokens_with_reserved_buffer() {
        let window = ContextWindow {
            context_tokens: 100_000,
            input_tokens: Some(80_000),
            output_token_max: 16_000,
            reserved_tokens: Some(5_000),
        };

        assert_eq!(window.usable_tokens(), 75_000);
    }

    #[test]
    fn reports_overflow_against_estimated_session_tokens() {
        let mut session = Session::new();
        session
            .messages
            .push(ConversationMessage::user_text("x ".repeat(200)));

        let result = status(
            &session,
            ContextWindow {
                context_tokens: 100,
                input_tokens: Some(10),
                output_token_max: 1,
                reserved_tokens: Some(0),
            },
            CompactionConfig::default(),
        );

        assert!(result.is_overflow);
    }
}
