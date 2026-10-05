/**
 * Tidy an order reference.
 *
 * @function Normalise
 * @param {STRING} text The reference as typed.
 * @returns {STRING} The reference without stray blanks.
 */
// The marker is the XML CDATA terminator, written inside a string on purpose.
const marker = "]]>";
const cleaned = text.replace(/\s+/g, " ").trim();
var result = cleaned.indexOf(marker) < 0 ? cleaned : cleaned + " é日本";