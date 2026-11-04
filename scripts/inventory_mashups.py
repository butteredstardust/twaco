#!/usr/bin/env python3
"""Inventory the mashups under one or more solution roots: what they use, and what twaco can author.

Reads managed sidecars (`src/<Mashup>/mashup/content.json`) and exported entity XML (`<mashupContent>` in a
Mashups folder). Read-only: inputs are never written. Output is deterministic JSON on stdout (or Markdown with
--markdown), so two runs over the same roots compare byte for byte.

    python scripts/inventory_mashups.py [--markdown] [--catalog widgets.json] <root>...

--catalog takes the output of `twaco widgets --json`; without it widget types are not classified.
"""

import argparse
import collections
import html
import json
import os
import re
import sys

# Agreed out of scope for authoring (not 10.2 catalogue widgets); reported, never counted as gaps.
OUT_OF_SCOPE = {"gridadvanced", "treegridadvanced", "collection", "googlemap", "infotableselector"}
# The root widget of every mashup, and the master-mashup root.
ROOT_TYPES = {"mashup", "targetmashup"}
MASHUP_CONTENT = re.compile(
    r"<mashupContent>\s*(?:<!\[CDATA\[(.*?)\]\]>|([^<]*))\s*</mashupContent>", re.S
)


def no_duplicate_keys(duplicates):
    def hook(pairs):
        seen = {}
        for key, value in pairs:
            if key in seen:
                duplicates.append(key)
            seen[key] = value  # last value wins, as serde_json does
        return seen

    return hook


def documents(root):
    """Yield (name, path, json text, has css sidecar or None) for every mashup under root, in a stable order."""
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames[:] = sorted(d for d in dirnames if d not in ("node_modules", "dist", ".git", "target"))
        for f in sorted(filenames):
            path = os.path.join(dirpath, f)
            if f == "content.json" and os.path.basename(dirpath) == "mashup":
                name = os.path.basename(os.path.dirname(dirpath))
                with open(path, encoding="utf-8-sig") as fh:
                    text = fh.read()
                css = os.path.join(dirpath, "custom.css")
                yield name, path, text, os.path.isfile(css) and os.path.getsize(css) > 0
            elif f.endswith(".xml") and "Mashups" in dirpath.replace("\\", "/").split("/"):
                with open(path, encoding="utf-8", errors="replace") as fh:
                    text = fh.read()
                names = re.findall(r'<Mashup\b[^>]*\bname="([^"]*)"', text)
                for i, m in enumerate(MASHUP_CONTENT.finditer(text)):
                    body = m.group(1) if m.group(1) is not None else html.unescape(m.group(2))
                    if body.strip():
                        name = names[i] if i < len(names) else os.path.splitext(f)[0]
                        yield name, path, body, None


def walk_widgets(node, out):
    if not isinstance(node, dict):
        return
    props = node.get("Properties") or {}
    out.append(props)
    for child in node.get("Widgets") or []:
        walk_widgets(child, out)


def inspect(doc):
    widgets = []
    walk_widgets(doc.get("UI") or {}, widgets)
    types = [w.get("Type") for w in widgets if w.get("Type")]
    structured = sorted(
        {
            f"{w.get('Type')}.{k}"
            for w in widgets
            for k, v in w.items()
            if isinstance(v, (dict, list)) and v not in ({}, []) and w.get("Type") not in ROOT_TYPES
        }
    )
    root = widgets[0] if widgets else {}
    sections = []
    for key, section in sorted((doc.get("Data") or {}).items()):
        if not isinstance(section, dict):
            continue
        sections.append(
            {
                "kind": section.get("EntityType") or key.split("_", 1)[0],
                "calls": len(section.get("Services") or []),
                "cached": sum(1 for s in section.get("Services") or [] if (s.get("Caching") or {}).get("cache")),
            }
        )
    entity_slots = sum(
        1
        for b in doc.get("DataBindings") or []
        if b.get("TargetArea") == "Data" and b.get("TargetId") == "EntityName"
    )
    return {
        "types": types,
        "structured": structured,
        "sections": sections,
        "entity_bindings": entity_slots,
        "bindings": len(doc.get("DataBindings") or []),
        "events": len(doc.get("Events") or []),
        "root_style": bool(root.get("StyleProperties")),
        "root_class": bool(root.get("CustomClass")),
        # Top-level in an export; a twaco workspace moves it to the custom.css sidecar.
        "css": bool((doc.get("CustomMashupCss") or "").strip()),
    }


def classify(t, catalog):
    if t in ROOT_TYPES:
        return "root"
    if t in OUT_OF_SCOPE:
        return "out_of_scope"
    if catalog is None:
        return "unclassified"
    return "catalogue" if t in catalog else "unknown"


def inventory(roots, catalog):
    mashups, malformed, seen, copies = [], [], set(), 0
    for root in roots:
        for name, path, text, css_sidecar in documents(root):
            # Build trees hold copies of the same entity; count each mashup once (first path wins).
            if name in seen:
                copies += 1
                continue
            seen.add(name)
            duplicates = []
            try:
                doc = json.loads(text, object_pairs_hook=no_duplicate_keys(duplicates))
            except ValueError as e:
                malformed.append({"mashup": name, "error": str(e).split(":")[0]})
                continue
            facts = inspect(doc)
            if css_sidecar:
                facts["css"] = True
            facts["mashup"] = name
            facts["duplicate_keys"] = sorted(set(duplicates))
            mashups.append(facts)
    mashups.sort(key=lambda m: m["mashup"])

    types = collections.Counter()
    type_mashups = collections.defaultdict(set)
    kinds = collections.Counter()
    kind_mashups = collections.defaultdict(set)
    structured = collections.Counter()
    for m in mashups:
        for t in m["types"]:
            types[t] += 1
            type_mashups[t].add(m["mashup"])
        for s in m["sections"]:
            kinds[s["kind"]] += 1
            kind_mashups[s["kind"]].add(m["mashup"])
        for p in m["structured"]:
            structured[p] += 1
    out_of_scope = sorted(m["mashup"] for m in mashups if any(t in OUT_OF_SCOPE for t in m["types"]))
    return {
        "mashups": len(mashups),
        "copies_skipped": copies,
        "malformed": sorted(malformed, key=lambda x: x["mashup"]),
        "duplicate_keys": sorted({m["mashup"] for m in mashups if m["duplicate_keys"]}),
        "out_of_scope_mashups": out_of_scope,
        "widget_types": [
            {"type": t, "widgets": types[t], "mashups": len(type_mashups[t]), "class": classify(t, catalog)}
            for t in sorted(types, key=lambda t: (-types[t], t))
        ],
        "data_sections": [
            {"kind": k, "sections": kinds[k], "mashups": len(kind_mashups[k])}
            for k in sorted(kinds, key=lambda k: (-kinds[k], k))
        ],
        "structured_properties": [
            {"property": p, "mashups": structured[p]} for p in sorted(structured, key=lambda p: (-structured[p], p))
        ],
        "root_style": {
            "StyleProperties": sum(m["root_style"] for m in mashups),
            "CustomClass": sum(m["root_class"] for m in mashups),
            "CustomMashupCss": sum(m["css"] for m in mashups),
        },
        "dynamic_entity_bindings": sum(m["entity_bindings"] for m in mashups),
        "cached_calls": sum(s["cached"] for m in mashups for s in m["sections"]),
    }


def markdown(report, top):
    lines = [f"# Mashup inventory: {report['mashups']} mashups", ""]
    lines += [f"- copies skipped (same name): {report['copies_skipped']}"]
    lines += [f"- malformed: {len(report['malformed'])}", f"- with duplicate keys: {len(report['duplicate_keys'])}"]
    lines += [f"- using out-of-scope widgets: {len(report['out_of_scope_mashups'])}", ""]
    lines += ["## Widget types", "", "| type | widgets | mashups | class |", "| --- | --- | --- | --- |"]
    lines += [f"| {w['type']} | {w['widgets']} | {w['mashups']} | {w['class']} |" for w in report["widget_types"]]
    lines += ["", "## Data sections", "", "| kind | sections | mashups |", "| --- | --- | --- |"]
    lines += [f"| {d['kind']} | {d['sections']} | {d['mashups']} |" for d in report["data_sections"]]
    lines += ["", f"dynamic entity bindings: {report['dynamic_entity_bindings']}; cached calls: {report['cached_calls']}"]
    lines += ["", f"## Structured properties (top {top})", "", "| property | mashups |", "| --- | --- |"]
    lines += [f"| {p['property']} | {p['mashups']} |" for p in report["structured_properties"][:top]]
    r = report["root_style"]
    lines += ["", "## Root style", "", f"StyleProperties {r['StyleProperties']}, CustomClass {r['CustomClass']}, "
              f"CustomMashupCss {r['CustomMashupCss']}", ""]
    return "\n".join(lines)


def main(argv):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("roots", nargs="+")
    parser.add_argument("--catalog", help="output of `twaco widgets --json`")
    parser.add_argument("--markdown", action="store_true")
    parser.add_argument("--top", type=int, default=40)
    args = parser.parse_args(argv)
    catalog = None
    if args.catalog:
        with open(args.catalog, encoding="utf-8-sig") as fh:
            catalog = {w["type"] for w in json.load(fh)}
    report = inventory(args.roots, catalog)
    if args.markdown:
        sys.stdout.write(markdown(report, args.top))
    else:
        sys.stdout.write(json.dumps(report, indent=2, sort_keys=True) + "\n")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
