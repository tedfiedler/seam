// tiny local module for the JS callback demo — seam callbacks arrive as
// promise-returning functions, so JS code awaits them like any async fn
module.exports = {
  twice: async (f, x) => f(await f(x)),
  primes: new Set([2, 3, 5, 7]),
  Accumulator: class {
    constructor(start) { this.total = start; }
    add(n) { this.total += n; return this.total; }
  },
};
