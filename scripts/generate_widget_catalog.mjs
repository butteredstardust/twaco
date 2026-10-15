// Generates src/core/widgets/catalog.json from a ThingWorx 10.2 installation. Run by hand:
//
//   node scripts/generate_widget_catalog.mjs <path to the Thingworx webapp folder>
//
// Reads Common/widget-meta-values.json (defaults) and Common/mashup-common-widgets-shared.js (each
// `widgetWrapper.config(...)` literal, evaluated in isolation), plus widget_catalog_overlay.json.

import { createHash } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import vm from "node:vm";

const here = dirname(fileURLToPath(import.meta.url));
const repo = join(here, "..");
const webapp = process.argv[2];
if (!webapp) {
  console.error("usage: node scripts/generate_widget_catalog.mjs <Thingworx webapp folder>");
  process.exit(2);
}

const metaBytes = readFileSync(join(webapp, "Common", "widget-meta-values.json"));
const sharedBytes = readFileSync(join(webapp, "Common", "mashup-common-widgets-shared.js"));
const meta = JSON.parse(metaBytes.toString("utf8"));
const sharedText = sharedBytes.toString("utf8");
const overlay = JSON.parse(readFileSync(join(here, "widget_catalog_overlay.json"), "utf8"));

const sha256 = (bytes) => createHash("sha256").update(bytes).digest("hex");

/** The balanced `{...}` that starts at `start`, skipping over string and template literals. */
function objectAt(text, start) {
  if (text[start] !== "{") throw new Error(`no object at ${start}`);
  let depth = 0;
  for (let i = start; i < text.length; i++) {
    const c = text[i];
    if (c === '"' || c === "'" || c === "`") {
      for (i++; i < text.length && text[i] !== c; i++) {
        if (text[i] === "\\") i++;
      }
      continue;
    }
    if (c === "{") depth++;
    else if (c === "}" && --depth === 0) return text.slice(start, i + 1);
  }
  throw new Error(`unbalanced object at ${start}`);
}

/** Evaluate an object literal with every free name bound to an inert stand-in. */
function evaluate(literal) {
  const inert = new Proxy(function () {}, {
    get: (_, key) => (key === Symbol.toPrimitive ? () => "" : inert),
    apply: () => inert,
    construct: () => inert,
  });
  const scope = new Proxy(
    {},
    {
      has: (_, key) => key !== "undefined",
      get: (_, key) => (key === Symbol.unscopables ? undefined : inert),
    },
  );
  return vm.runInNewContext(`with (__scope) { (${literal}) }`, { __scope: scope }, { timeout: 1000 });
}

const contracts = {};
for (const match of sharedText.matchAll(/widgetWrapper\.config\("([a-z0-9]+)",\{/g)) {
  const type = match[1];
  if (contracts[type]) throw new Error(`${type} is configured twice`);
  contracts[type] = evaluate(objectAt(sharedText, match.index + match[0].length - 1));
}

const plain = (value) =>
  value === null || ["string", "number", "boolean"].includes(typeof value) ? value : undefined;

function property(definition) {
  const out = { base_type: typeof definition.baseType === "string" ? definition.baseType : null };
  // The widget API's default for both flags is false.
  if (definition.isBindingSource === true) out.source = true;
  if (definition.isBindingTarget === true) out.target = true;
  if (definition.isEditable === false) out.editable = false;
  if (Array.isArray(definition.selectOptions)) {
    const options = definition.selectOptions
      .map((option) => plain(option?.value))
      .filter((value) => value !== undefined);
    if (options.length) out.options = options;
  }
  return out;
}

function refuseCaseClash(keys, type) {
  const seen = new Map();
  for (const key of keys) {
    const folded = key.toLowerCase();
    if (seen.has(folded)) throw new Error(`${type}: keys ${seen.get(folded)} and ${key} differ only in case`);
    seen.set(folded, key);
  }
}

const widgets = [];
const named = new Map();
for (const [type, extra] of Object.entries(overlay.widgets)) {
  const defaults = meta[type]?.Properties;
  if (!defaults) throw new Error(`overlay names ${type}, which widget-meta-values.json does not have`);
  refuseCaseClash(Object.keys(defaults), type);
  // Composer translates `__tw.*` keys and resolves `$$...` markers when it drops a widget; the
  // overlay gives the value it writes, so none reaches a mashup untranslated.
  Object.assign(defaults, extra.overrides ?? {});
  for (const [key, value] of Object.entries(defaults)) {
    if (typeof value === "string" && (value.startsWith("__tw.") || value.startsWith("$$"))) {
      throw new Error(`${type}.${key} is ${value}, which Composer translates; give its value in overrides`);
    }
  }
  for (const word of [type, ...(extra.aliases ?? [])]) {
    if (named.has(word)) throw new Error(`${word} names both ${named.get(word)} and ${type}`);
    named.set(word, type);
  }
  const entry = {
    type,
    aliases: extra.aliases ?? [],
    type_display_name: extra.type_display_name,
    // Composer's DisplayName for a dropped widget: ptcsbutton -> ptcs-button-<n>.
    display_name: extra.display_name ?? (type.startsWith("ptcs") ? `ptcs-${type.slice(4)}` : type),
    container: extra.container === true,
    defaults,
  };
  const contract = contracts[type];
  if (contract) {
    const properties = {};
    for (const [name, definition] of Object.entries(contract.properties ?? {})) {
      if (definition && typeof definition === "object") properties[name] = property(definition);
    }
    refuseCaseClash(Object.keys(properties), type);
    entry.properties = properties;
    entry.events = Object.keys(contract.events ?? {});
    entry.services = Object.keys(contract.services ?? {});
  }
  widgets.push(entry);
}

const catalog = {
  platform: "10.2",
  generated_by: "scripts/generate_widget_catalog.mjs",
  source: {
    widget_meta_values: { path: "Common/widget-meta-values.json", sha256: sha256(metaBytes) },
    widget_contracts: { path: "Common/mashup-common-widgets-shared.js", sha256: sha256(sharedBytes) },
  },
  base_properties: overlay.base_properties,
  dropped: { leaf: overlay.dropped.leaf, flexcontainer: overlay.dropped.flexcontainer },
  widgets,
};

const out = join(repo, "src", "core", "widgets", "catalog.json");
writeFileSync(out, JSON.stringify(catalog, null, 2) + "\n");
console.error(`${widgets.length} widgets (${Object.keys(contracts).length} contracts in the bundle) -> ${out}`);
