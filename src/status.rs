use crate::model::Status;

pub const WORKING_SECONDS: i64 = 10 * 60;
pub const DONE_SECONDS: i64 = 60 * 60;

#[derive(Debug, Default)]
pub struct Tail {
    pub pending: bool,
    pub ended: bool,
    pub text: String,
    pub explicit_error: bool,
}

/// PLAN §6 precedence is intentional: an ended turn wins over an earlier error.
pub fn derive(tail: &Tail, last_activity: i64, now: i64) -> Status {
    let age = now.saturating_sub(last_activity).max(0);
    if tail.pending {
        return if age < WORKING_SECONDS {
            Status::Working
        } else {
            Status::Error
        };
    }
    if tail.ended {
        if asks(&tail.text) {
            return Status::Waiting;
        }
        return if age < DONE_SECONDS {
            Status::Done
        } else {
            Status::Idle
        };
    }
    if tail.explicit_error {
        return Status::Error;
    }
    Status::Idle
}

fn asks(text: &str) -> bool {
    let text = text.trim_end();
    text.ends_with('?') || text.contains("<request_user_input>") || text.contains("<question>")
}
