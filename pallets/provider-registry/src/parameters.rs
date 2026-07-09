//! Pure default constants for `pallet-provider-registry` (D.1 rule 9).
//!
//! Balance-denominated defaults are expressed as *multiples of the existential
//! deposit*; the runtime scales them by its `EXISTENTIAL_DEPOSIT` when building
//! the `#[pallet::constant]` value (spec D.8). Nothing here reads runtime state.

/// Minimum stake to register, in units of the existential deposit (spec D.8).
pub const MIN_STAKE_ED_MULTIPLE: u128 = 1_000;
