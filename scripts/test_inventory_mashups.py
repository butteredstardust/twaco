import json
import os
import tempfile
import unittest

import inventory_mashups as inv

CONTENT = {
    "Data": {
        "DynamicThingShapes_Acme.Orders_TS": {
            "DataName": "DynamicThingShapes_Acme.Orders_TS",
            "EntityName": "Acme.Orders_TS",
            "EntityType": "DynamicThingShapes",
            "Services": [{"Name": "GetOrders", "Target": "GetOrders"}],
        },
        "Session": {"EntityType": "Session", "Services": []},
    },
    "DataBindings": [
        {"SourceArea": "Mashup", "TargetArea": "Data", "TargetId": "EntityName", "PropertyMaps": []},
    ],
    "Events": [],
    "UI": {
        "Properties": {"Type": "mashup", "StyleProperties": {"a": 1}},
        "Widgets": [
            {
                "Properties": {"Type": "flexcontainer"},
                "Widgets": [
                    {"Properties": {"Type": "ptcsgrid", "ColumnFormat": {"formatInfo": [1]}}, "Widgets": []},
                    {"Properties": {"Type": "gridadvanced"}, "Widgets": []},
                    {"Properties": {"Type": "acmewidget"}, "Widgets": []},
                ],
            }
        ],
    },
}


def write(path, text):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8") as fh:
        fh.write(text)


class InventoryTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = self.tmp.name

    def tearDown(self):
        self.tmp.cleanup()

    def sidecar(self, name, content, css=""):
        write(os.path.join(self.root, "src", name, "mashup", "content.json"), json.dumps(content))
        write(os.path.join(self.root, "src", name, "mashup", "custom.css"), css)

    def export(self, name, content, folder="Mashups"):
        body = json.dumps(dict(content, CustomMashupCss=".x {}"))
        write(
            os.path.join(self.root, folder, name + ".xml"),
            f'<Entities><Mashups><Mashup name="{name}"><mashupContent><![CDATA[{body}]]></mashupContent>'
            "</Mashup></Mashups></Entities>",
        )

    def report(self, catalog=frozenset({"flexcontainer", "ptcsgrid"})):
        return inv.inventory([self.root], set(catalog))

    def test_sidecar_and_export_give_the_same_facts(self):
        self.sidecar("Acme.A_MU", CONTENT, css=".x {}")
        a = self.report()
        self.tearDown()
        self.setUp()
        self.export("Acme.A_MU", CONTENT)
        b = self.report()
        self.assertEqual(a, b)
        self.assertEqual(a["mashups"], 1)
        self.assertEqual(a["root_style"]["CustomMashupCss"], 1)

    def test_counts_sections_entity_bindings_and_structured_properties(self):
        self.sidecar("Acme.A_MU", CONTENT)
        r = self.report()
        kinds = {d["kind"]: d["sections"] for d in r["data_sections"]}
        self.assertEqual(kinds, {"DynamicThingShapes": 1, "Session": 1})
        self.assertEqual(r["dynamic_entity_bindings"], 1)
        self.assertEqual(r["structured_properties"], [{"property": "ptcsgrid.ColumnFormat", "mashups": 1}])
        self.assertEqual(r["root_style"]["CustomMashupCss"], 0)  # empty custom.css is no CSS

    def test_classifies_legacy_widgets_apart_from_unknown_ones(self):
        self.sidecar("Acme.A_MU", CONTENT)
        classes = {w["type"]: w["class"] for w in self.report()["widget_types"]}
        self.assertEqual(classes["gridadvanced"], "out_of_scope")
        self.assertEqual(classes["acmewidget"], "unknown")
        self.assertEqual(classes["ptcsgrid"], "catalogue")
        self.assertEqual(classes["mashup"], "root")
        self.assertEqual(self.report()["out_of_scope_mashups"], ["Acme.A_MU"])

    def test_counts_a_mashup_once_when_a_build_tree_copies_it(self):
        self.export("Acme.A_MU", CONTENT)
        self.export("Acme.A_MU", CONTENT, folder=os.path.join("build", "Mashups"))
        r = self.report()
        self.assertEqual((r["mashups"], r["copies_skipped"]), (1, 1))

    def test_the_same_name_in_two_roots_is_two_mashups(self):
        self.sidecar("Acme.A_MU", CONTENT)
        other = tempfile.TemporaryDirectory()
        self.addCleanup(other.cleanup)
        write(os.path.join(other.name, "src", "Acme.A_MU", "mashup", "content.json"), json.dumps(CONTENT))
        r = inv.inventory([self.root, other.name], {"flexcontainer", "ptcsgrid"})
        self.assertEqual((r["mashups"], r["copies_skipped"]), (2, 0))

    def test_reports_duplicate_keys_and_malformed_json(self):
        write(os.path.join(self.root, "src", "Acme.Dup_MU", "mashup", "content.json"), '{"UI": {}, "UI": {}}')
        write(os.path.join(self.root, "src", "Acme.Bad_MU", "mashup", "content.json"), "{not json")
        r = self.report()
        self.assertEqual(r["duplicate_keys"], ["Acme.Dup_MU"])
        self.assertEqual([m["mashup"] for m in r["malformed"]], ["Acme.Bad_MU"])

    def test_output_is_stable(self):
        self.sidecar("Acme.B_MU", CONTENT)
        self.sidecar("Acme.A_MU", CONTENT)
        first = json.dumps(self.report(), sort_keys=True)
        self.assertEqual(first, json.dumps(self.report(), sort_keys=True))


if __name__ == "__main__":
    unittest.main()
