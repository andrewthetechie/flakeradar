//! The three tests the sample's `quarantine.json` names. Two of them,
//! `adds_an_item` and `checkout_total`, are what the fixture quarantines, so
//! `adds_an_item_twice` is the only one a filterset built from that fixture
//! leaves running.

use quarantine_example::cart_total;

#[test]
fn adds_an_item() {
    assert_eq!(cart_total(&[350]), 350);
}

#[test]
fn adds_an_item_twice() {
    assert_eq!(cart_total(&[350, 350]), 700);
}

#[test]
fn checkout_total() {
    assert_eq!(cart_total(&[350, 120, 499]), 969);
}
