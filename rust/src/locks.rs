//! Poison recovery for locks protecting containers and coordination state that
//! remain valid after unwinding. Resources needing rollback or reset recover at
//! their owner instead, before they can be reused.
use std::sync::{LockResult, PoisonError};

pub(crate) trait RecoverLock<T> {
    fn recover(self) -> T;
}

impl<T> RecoverLock<T> for LockResult<T> {
    fn recover(self) -> T {
        self.unwrap_or_else(PoisonError::into_inner)
    }
}
