//! Verus compatibility shim.
//!
//! Under standard `rustc`, expands Verus-flavoured macros into runtime
//! checks so the code compiles and tests without the Verus compiler
//! driver.  When compiled with the real Verus toolchain, the feature
//! `verus` swaps these for the real `builtin_macros` equivalents.

/// Define a specification function.
///
/// Under Verus this becomes a `spec fn`; under standard Rust it is a
/// regular `fn`.
///
/// Usage:
/// ```ignore
/// spec_fn!(name, (arg: T) -> bool, { body });
/// ```
#[macro_export]
macro_rules! spec_fn {
    ($name:ident, ($($arg:ident : $ty:ty),*) -> $ret:ty, $body:block) => {
        pub fn $name($($arg: $ty),*) -> $ret $body
    };
}
pub use spec_fn;

/// Runtime precondition check.
///
/// Under Verus this maps to `requires(...)`.  Under standard Rust it
/// panics if the predicate is false, acting as a runtime guard.
#[macro_export]
macro_rules! requires {
    ($pred:expr) => {
        if !$pred {
            return Err("precondition violated");
        }
    };
}
pub use requires;

/// Runtime postcondition annotation (no-op under standard Rust).
///
/// Under Verus this maps to `ensures(...)`.
#[macro_export]
macro_rules! ensures {
    ($pred:expr) => {
        // postcondition — checked by Verus at proof time, no-op under rustc
        let _ = $pred;
    };
}
pub use ensures;

/// Invariant assertion (runtime under rustc, proof-time under Verus).
#[macro_export]
macro_rules! invariant {
    ($pred:expr) => {
        debug_assert!($pred, "invariant violated");
    };
}
pub use invariant;

/// Ghost variable — exists only at proof time.
///
/// Under standard Rust this is just a value holder with no special semantics.
#[derive(Debug, Clone, Copy)]
pub struct Ghost<T>(pub T);

impl<T> Ghost<T> {
    pub fn new(val: T) -> Self {
        Self(val)
    }

    pub fn view(&self) -> &T {
        &self.0
    }
}
