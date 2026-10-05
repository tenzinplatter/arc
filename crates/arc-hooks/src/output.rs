use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum EventName {
    PostToolUse,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HookSpecificOutput {
    pub hook_event_name: EventName,
    pub additional_context: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextOutput {
    pub hook_specific_output: HookSpecificOutput,
}

impl ContextOutput {
    pub fn post_tool_use(additional_context: String) -> Self {
        Self {
            hook_specific_output: HookSpecificOutput {
                hook_event_name: EventName::PostToolUse,
                additional_context,
            },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Decision {
    Block,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct BlockOutput {
    pub decision: Decision,
    pub reason: String,
}

impl BlockOutput {
    pub fn block(reason: String) -> Self {
        Self {
            decision: Decision::Block,
            reason,
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn outputs_match_the_documented_shapes() {
        assert_eq!(
            serde_json::to_value(ContextOutput::post_tool_use("linked".into())).unwrap(),
            json!({"hookSpecificOutput": {"hookEventName": "PostToolUse", "additionalContext": "linked"}})
        );
        assert_eq!(
            serde_json::to_value(BlockOutput::block("log it".into())).unwrap(),
            json!({"decision": "block", "reason": "log it"})
        );
    }
}
