import { createHash } from "node:crypto";
import { readFileSync, readdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";

// Split one minified Composer object or array into its immediate members. We inspect
// callbacks as text so that neither they nor their matchers run in the generator.
function members(text, start) {
  const closing = { "{": "}", "[": "]", "(": ")" };
  const stack = [closing[text[start]]];
  const parts = [];
  let member = start + 1;
  for (let i = member; i < text.length; i++) {
    const char = text[i];
    if (char === '"' || char === "'" || char === "`") {
      for (i++; i < text.length && text[i] !== char; i++) {
        if (text[i] === "\\") i++;
      }
    } else if (Object.hasOwn(closing, char)) {
      stack.push(closing[char]);
    } else if (char === stack.at(-1)) {
      stack.pop();
      if (stack.length === 0) {
        if (member < i) parts.push(text.slice(member, i));
        return parts;
      }
    } else if (char === "," && stack.length === 1) {
      parts.push(text.slice(member, i));
      member = i + 1;
    }
  }
  throw new Error("unclosed Composer migration literal");
}

// Only the literal subset of JavaScript used in these tables. Quote bare keys,
// then let JSON.parse reject identifiers, expressions and function calls.
function literal(text) {
  if (!text) return undefined;
  const json = text.replace(/"(?:\\.|[^"\\])*"|[\w$]+(?=\s*:)/g, (word) =>
    word.startsWith('"') ? word : JSON.stringify(word),
  );
  try {
    return JSON.parse(json);
  } catch {
    return undefined;
  }
}

function fields(text) {
  return Object.fromEntries(members(text, 0).map((part) => {
    const colon = part.indexOf(":");
    return [part.slice(0, colon), part.slice(colon + 1)];
  }));
}

// These shipped conversions used only the first declarative newtype step. Keep
// subsequent Composer steps out of their generated rules to preserve their output.
const original = new Set([
  "checkbox", "collection", "container", "divider", "image", "label",
  "link", "numericentry", "textarea", "textbox",
]);

const requiresCallbacks = {
  dataexport: "creates an export function and rewires a menu around it",
  expression: "rewrites expressions and their dynamic parameter connections",
  labelchart: "chooses a target and converts series in callbacks",
  logoutbutton: "creates and wires a logout function",
  navigation: "creates and wires a navigation function",
  tabs: "creates tab definitions and reparents children",
  tabsv2: "rebuilds tab definitions and child slots",
  timeserieschart: "chooses a target and converts series in callbacks",
  validator: "rewrites expressions and their dynamic parameter connections",
  xychart: "chooses a target and converts series in callbacks",
};

function merge(rule, step) {
  for (const name of ["create", "mappings"]) {
    if (step[name]) rule[name] = { ...rule[name], ...step[name] };
  }
  if (step.deletions) rule.deletions = [...new Set([...(rule.deletions ?? []), ...step.deletions])];
  if (step.deletionsWC) {
    rule.delete_patterns = [...new Set([...(rule.delete_patterns ?? []), ...[step.deletionsWC].flat()])];
  }
}

function* composerTables(text) {
  for (const module of text.split('define("')) {
    const type = /^[^"]*\/thingworx-ide-mig\w*-mappings-(\w+)"/.exec(module)?.[1];
    if (!type) continue;
    const exported = /\.get\w+MUs=(\w+);/.exec(module)?.[1];
    if (!exported) continue;
    const body = module.indexOf(`function ${exported}(){`);
    if (body < 0) continue;
    if (type === "autoupdate") {
      // Most entries update a widget in place. A few change its actual type.
      const returned = module.indexOf("return{", body);
      for (const [source, entries] of Object.entries(fields(module.slice(returned + "return".length)))) {
        yield [source, members(entries, 0), true];
      }
    } else {
      const returned = module.indexOf("return[", body);
      if (returned >= 0) yield [type, members(module, returned + "return".length), false];
    }
  }
}

function composerRules(text, types) {
  const rules = {};
  const skipped = {};
  for (const [type, entries, updates] of composerTables(text)) {
    if (Object.hasOwn(rules, type) || Object.hasOwn(skipped, type)) continue;
    if (Object.hasOwn(requiresCallbacks, type)) {
      skipped[type] = requiresCallbacks[type];
      continue;
    }
    const steps = entries.flatMap((entry) => {
      if (!entry.startsWith("{")) return [];
      const { matcher, migrator } = fields(entry);
      if (!migrator?.startsWith("{")) return [];
      const step = literal(migrator);
      if (!step) throw new Error(`${type}: expected a literal migrator: ${migrator}`);
      return [{ step, matcher, when: literal(matcher) }];
    });
    const variants = [];
    for (const { step, matcher, when } of steps) {
      if (!step.newtype || step.newtype === type) continue;
      if (!types.has(step.newtype)) {
        skipped[`${type} → ${step.newtype}`] = "target absent from the catalogue";
        continue;
      }
      const matchable = when && !Array.isArray(when) && typeof when === "object";
      if (matcher && !matchable && type !== "container") continue;
      const rule = { newtype: step.newtype };
      if (matchable) rule.when = when;
      if (original.has(type)) {
        merge(rule, step);
      } else {
        const conditional = [];
        for (const candidate of steps) {
          if (candidate.step === step || (!candidate.step.newtype && !candidate.matcher)) {
            merge(rule, candidate.step);
          } else if (!candidate.step.newtype && candidate.when
                     && !Array.isArray(candidate.when) && typeof candidate.when === "object") {
            const edit = { when: candidate.when };
            merge(edit, candidate.step);
            conditional.push(edit);
          }
        }
        if (conditional.length) rule.conditional = conditional;
      }
      variants.push(rule);
    }
    if (variants.length === 1 && !variants[0].when) {
      rules[type] = variants[0];
    } else if (variants.length) {
      rules[type] = { variants };
    } else if (!updates) {
      skipped[type] = "no statically selectable catalogue newtype";
    }
  }
  for (const type of original) {
    if (!rules[type]) throw new Error(`missing the existing ${type} migration`);
  }
  return {
    newtypes: Object.fromEntries(Object.keys(rules).sort().map((type) => [type, rules[type]])),
    skipped_newtypes: Object.fromEntries(Object.keys(skipped).sort().map((type) => [type, skipped[type]])),
  };
}

export function generateMigrations(webapp, repo, widgets) {
  const scripts = join(webapp, "Composer", "scripts");
  const bundles = readdirSync(scripts).filter((file) => /^twx-Builder-bundle-.*\.js$/.test(file));
  if (bundles.length !== 1) throw new Error("expected one Composer Builder bundle");
  const bundle = readFileSync(join(scripts, bundles[0]));
  const overlay = JSON.parse(readFileSync(join(repo, "scripts", "widget_migrations_overlay.json"), "utf8"));
  const types = new Set(widgets.map((widget) => widget.type));
  const { newtypes, skipped_newtypes } = composerRules(bundle.toString("utf8"), types);
  for (const [source, rule] of Object.entries(overlay.twaco_newtypes)) {
    if (!types.has(rule.newtype)) throw new Error(`${source}: twaco migration target is absent from the catalogue`);
    if (Object.hasOwn(newtypes, source)) throw new Error(`${source}: Composer now has a migration for this widget`);
  }
  const migrations = {
    source: overlay.source,
    renames: overlay.renames,
    values_differ: overlay.values_differ,
    newtypes_source: {
      platform: "10.2",
      path: `Composer/scripts/${bundles[0]}`,
      bundle_sha256: createHash("sha256").update(bundle).digest("hex"),
      preserve_wiring: [...original].sort(),
      note: "Declarative Composer newtype steps. Collection and repeater configuration, numeric input mode, container layout, grid column options, pie labels, upload limits, and hidden auto-refresh functions use callback translations in authoring/migrate.rs. Theme style callbacks are omitted.",
    },
    newtypes,
    skipped_newtypes,
    twaco_newtypes: overlay.twaco_newtypes,
  };
  writeFileSync(join(repo, "src", "core", "widgets", "migrations.json"), JSON.stringify(migrations, null, 2) + "\n");
  console.error(`${Object.keys(newtypes).length} Composer newtype families; ${Object.keys(skipped_newtypes).length} skipped`);
}
