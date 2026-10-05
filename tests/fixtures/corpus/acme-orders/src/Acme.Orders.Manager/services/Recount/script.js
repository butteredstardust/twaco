/**
 * How many settings the manager has, and how many orders it handled.
 *
 * @function Recount
 * @returns {INTEGER} The number of settings rows.
 */
const settings = me.GetConfigurationTable({ tableName: "Settings" });
me.OrderCount = settings.rows.length;
var result = settings.rows.length;