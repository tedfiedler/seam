// tiny local module for the JS callback demo — seam callbacks arrive as
// promise-returning functions, so JS code awaits them like any async fn
module.exports = {
  twice: async (f, x) => f(await f(x)),
};
