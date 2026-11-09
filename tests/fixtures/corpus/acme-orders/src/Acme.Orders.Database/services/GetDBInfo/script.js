// Overrides GetDBInfo.
var result = {
    dbInfo: [
        {
            dataShapeName: "Acme.Orders.OrderLine_DS",
            fields: [
                { name: "orderId", notNull: true },
                { name: "sku", notNull: true },
                { name: "quantity" }
            ],
            indexedFields: [
                { name: "orderId" },
                { fieldNames: ["orderId", "sku"], unique: true }
            ],
            foreignKeys: []
        }
    ]
};