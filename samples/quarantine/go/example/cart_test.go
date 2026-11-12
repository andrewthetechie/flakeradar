// The three tests the sample's quarantine.json names, in the module's one
// package. Two of them, TestAddsAnItem and TestCheckoutTotal, are what the
// fixture quarantines, so TestAddsAnItemTwice is the only one a -skip pattern
// built from that fixture leaves running. The subtest with_discount is what
// gotestsum writes as the name TestCheckoutTotal/with_discount, which
// go test -skip cannot skip alone.
package cart

import "testing"

// cartTotal sums the prices, in cents, of the items added to a cart.
func cartTotal(prices []int) int {
	total := 0
	for _, price := range prices {
		total += price
	}
	return total
}

// discounted returns the total, in cents, after a discount in cents.
func discounted(total, discount int) int {
	return total - discount
}

func TestAddsAnItem(t *testing.T) {
	if got := cartTotal([]int{350}); got != 350 {
		t.Errorf("cartTotal([]int{350}) = %d, want 350", got)
	}
}

func TestAddsAnItemTwice(t *testing.T) {
	if got := cartTotal([]int{350, 350}); got != 700 {
		t.Errorf("cartTotal([]int{350, 350}) = %d, want 700", got)
	}
}

func TestCheckoutTotal(t *testing.T) {
	if got := cartTotal([]int{350, 120, 499}); got != 969 {
		t.Errorf("cartTotal([]int{350, 120, 499}) = %d, want 969", got)
	}

	t.Run("with_discount", func(t *testing.T) {
		if got := discounted(969, 100); got != 869 {
			t.Errorf("discounted(969, 100) = %d, want 869", got)
		}
	})
}
