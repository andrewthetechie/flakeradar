//! The smallest crate that gives the sample's three tests something to check:
//! one public function that totals a cart, driven from `tests/cart.rs`.

/// Sums the prices, in cents, of the items added to a cart.
pub fn cart_total(prices: &[u32]) -> u32 {
    prices.iter().sum()
}
