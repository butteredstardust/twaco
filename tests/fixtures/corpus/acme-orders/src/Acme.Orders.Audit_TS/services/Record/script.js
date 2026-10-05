/**
 * Write one audit line.
 *
 * @function Record
 * @param {STRING} orderId The order.
 * @param {STRING} note What happened.
 */
logger.info("order " + orderId + ": " + note);