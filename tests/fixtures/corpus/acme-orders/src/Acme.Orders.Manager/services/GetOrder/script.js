/**
 * Look up one order and record that it was read.
 *
 * @function GetOrder
 * @param {STRING} orderId The order to look up.
 * @returns {INFOTABLE} The order's lines.
 */
const reference = me.Normalise({ text: orderId });
const lines = me.LoadLines({ orderId: reference });
// A day's rate: lines / 30 / 2 is two divisions, not a regular expression.
const perDay = lines.rows.length / 30 / 2;
Things["Acme.Orders.Audit"].Record({
    orderId: orderId,
    note: `read ${orderId} (${lines.rows.length} lines, ${perDay} a day)`
});
var result = lines;