// Builds the ThingWorx extension zip, zip/<packageName>-<prod|dev>-v<version>.zip, from
// package.json: the widget bundles under ui/<packageName>/, src/static/ beside them,
// Entities/*.xml and a generated metadata.xml.
//
//   npm run build       minified
//   npm run buildDev    readable, with inline source maps
import * as esbuild from 'esbuild';
import { zipSync } from 'fflate';
import fs from 'node:fs';
import path from 'node:path';

const dev = process.argv.includes('--dev');
const pkg = JSON.parse(fs.readFileSync('package.json', 'utf8'));
const name = pkg.packageName;
if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(name ?? '')) {
    throw new Error(`package.json packageName ${JSON.stringify(name)} is not a widget package name`);
}
const ui = path.join('build', 'ui', name);
fs.rmSync('build', { recursive: true, force: true });
fs.mkdirSync(ui, { recursive: true });

await esbuild.build({
    entryPoints: {
        [`${name}.ide.bundle`]: 'src/ide/index.ts',
        [`${name}.runtime.bundle`]: 'src/runtime/index.ts',
    },
    outdir: ui,
    bundle: true,
    format: 'iife',
    target: 'es2017',
    minify: !dev,
    // typescriptwebpacksupport registers each widget under its class name.
    keepNames: true,
    sourcemap: dev ? 'inline' : false,
    loader: { '.svg': 'dataurl', '.png': 'dataurl', '.jpg': 'dataurl', '.gif': 'dataurl' },
    define: { VERSION: JSON.stringify(pkg.version) },
    logLevel: 'warning',
});

copy('src/static', path.join(ui, 'static'));
copy('Entities', path.join('build', 'Entities'), (file) => file.endsWith('.xml'));

const resources = [
    ['ide.bundle.js', 'JS', true],
    ['ide.bundle.css', 'CSS', true],
    ['runtime.bundle.js', 'JS', false],
    ['runtime.bundle.css', 'CSS', false],
]
    .filter(([suffix]) => fs.existsSync(path.join(ui, `${name}.${suffix}`)))
    .map(
        ([suffix, type, ide]) =>
            `        <FileResource type="${type}" file="${name}.${suffix}" description="" isDevelopment="${ide}" isRuntime="${!ide}"/>`,
    );
const attr = (value) =>
    String(value ?? '').replace(/&/g, '&amp;').replace(/"/g, '&quot;').replace(/</g, '&lt;');
fs.writeFileSync(
    path.join('build', 'metadata.xml'),
    `<?xml version="1.0" encoding="UTF-8"?>
<Entities>
  <ExtensionPackages>
    <ExtensionPackage name="${attr(name)}" description="${attr(pkg.description)}" vendor="${attr(pkg.author)}" packageVersion="${attr(pkg.version)}" minimumThingWorxVersion="${attr(pkg.minimumThingWorxVersion)}"/>
  </ExtensionPackages>
  <Widgets>
    <Widget name="${attr(name)}">
      <UIResources>
${resources.join('\n')}
      </UIResources>
    </Widget>
  </Widgets>
</Entities>
`,
);

const files = {};
walk('build', (file) => {
    files[path.relative('build', file).split(path.sep).join('/')] = fs.readFileSync(file);
});
fs.rmSync('zip', { recursive: true, force: true });
fs.mkdirSync('zip');
const zipName = `${name}-${dev ? 'dev' : 'prod'}-v${pkg.version}.zip`;
fs.writeFileSync(path.join('zip', zipName), zipSync(files));
console.log(`built zip/${zipName}`);

function copy(from, to, keep = () => true) {
    walk(from, (file) => {
        if (!keep(file)) return;
        const target = path.join(to, path.relative(from, file));
        fs.mkdirSync(path.dirname(target), { recursive: true });
        fs.copyFileSync(file, target);
    });
}

function walk(dir, visit) {
    if (!fs.existsSync(dir)) return;
    for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
        const full = path.join(dir, entry.name);
        if (entry.isDirectory()) walk(full, visit);
        else if (entry.isFile() && entry.name !== '.gitkeep') visit(full);
    }
}
