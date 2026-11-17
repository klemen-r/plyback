//! Account insight computations and immutable reports. This module never starts an engine.
mod compute;
mod reports;
pub(crate) use compute::LoadedGamesCache;
pub use compute::{compute, normalize_filters, scope_game_ids};
pub use reports::Insights;
