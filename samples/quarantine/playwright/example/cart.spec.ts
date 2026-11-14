// The sample suite this helper filters, mirroring the four names of
// samples/quarantine/vitest-jest/example/cart.test.js: the describe `cart` holds
// `adds an item` and `adds an item twice`, and the top level holds
// `checkout total` and `shows the empty cart`. The two items of
// ../quarantine.json quote `cart.spec.ts` / `cart › adds an item` and
// `cart.spec.ts` / `checkout total`, so a filtered run leaves the other two.
//
// Every body is a plain assertion, and no test takes the `page` fixture, so
// Playwright never starts a browser and the sample's image needs no
// `playwright install`.

import { expect, test } from "@playwright/test";

test.describe("cart", () => {
  test("adds an item", () => {
    const cart: string[] = [];
    cart.push("apple");
    expect(cart).toEqual(["apple"]);
  });

  test("adds an item twice", () => {
    const cart: string[] = [];
    cart.push("apple", "pear");
    expect(cart).toEqual(["apple", "pear"]);
  });
});

test("checkout total", { tag: "@slow" }, () => {
  expect(2 + 3).toBe(5);
});

test("shows the empty cart", () => {
  expect([].length).toBe(0);
});
