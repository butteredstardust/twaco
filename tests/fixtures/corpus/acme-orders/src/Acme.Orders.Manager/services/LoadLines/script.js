/**
 * The lines of one order.
 *
 * @function LoadLines
 * @param {STRING} orderId The order.
 * @returns {INFOTABLE} The lines.
 */
var result = Things["Acme.Orders.Lines_DT"].GetDataTableEntries({
    query: { filters: { type: "EQ", fieldName: "orderId", value: orderId } }
});