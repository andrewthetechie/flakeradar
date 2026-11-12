// The sample suite both runners execute. It uses globals only, with no imports,
// so Vitest (which needs globals: true in its config) and Jest (whose globals are
// always on) load the same file. The names are the ones the two quarantine
// fixtures refer to.
describe("cart", () => {
  test("adds an item", () => {
    const cart = [];
    cart.push("apple");
    expect(cart).toEqual(["apple"]);
  });

  test("adds an item twice", () => {
    const cart = [];
    cart.push("apple", "pear");
    expect(cart).toEqual(["apple", "pear"]);
  });
});

test("checkout total", () => {
  expect(2 + 3).toBe(5);
});

test("shows the empty cart", () => {
  expect([].length).toBe(0);
});
