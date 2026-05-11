//! `ask_user` — meta-tool the agent calls to pause the workflow and wait for
//! a free-text answer from the operator. Used when the agent legitimately
//! can't make a call by itself (multiple ambiguous Dockerfiles, which test
//! exercises the hot path, granting a privileged capability, etc.).
//!
//! Mechanics: this tool's `run()` calls into [`crate::user_input::ask`],
//! which parks a oneshot reply channel and emits the `agent:blocked` event.
//! The UI shows a modal; the operator's "Submit" invokes the
//! `answer_blocked_question` Tauri command which delivers the reply.
//! The agent loop continues as if the answer were a normal tool result.

use anyhow::Result;
use serde::{Deserialize, Serialize};

use super::ToolManifest;

pub const NAME: &str = "ask_user";
pub const DESCRIPTION: &str =
    "Pause the agent and ask the operator a free-text question. Use ONLY when you're \
     genuinely stuck and a human decision is required — multiple ambiguous Dockerfiles, \
     which test exercises the production hot path, whether to run with a privileged \
     capability, what command starts the service when there's no Dockerfile. Do not use \
     for confirmations the agent should make on its own. The returned `answer` field is \
     the operator's verbatim reply.";
pub const PARAMETERS: &str = r#"{
  "type": "object",
  "properties": {
    "question": {
      "type": "string",
      "description": "A specific, answerable question. Don't bundle multiple questions; ask one at a time."
    }
  },
  "required": ["question"]
}"#;

#[derive(Debug, Deserialize)]
pub struct Args {
    pub question: String,
}

#[derive(Debug, Serialize)]
pub struct Output {
    /// The operator's verbatim answer. Empty string is possible if the
    /// operator submitted with no text — treat that as "no preference".
    pub answer: String,
}

pub fn manifest() -> ToolManifest {
    ToolManifest {
        name: NAME,
        description: DESCRIPTION,
        parameters: PARAMETERS,
    }
}

pub async fn run(args: Args) -> Result<Output> {
    let q = args.question.trim();
    if q.is_empty() {
        anyhow::bail!("question must not be empty");
    }
    let answer = crate::user_input::ask(q.to_string())
        .await
        .map_err(|e| anyhow::anyhow!(e))?;
    Ok(Output { answer })
}
