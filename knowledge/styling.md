# Styling a mashup

Colour, font, spacing and custom CSS for a mashup: which twaco command changes which layer.

## Style, colour, custom CSS or theme: which layer

- A widget's own look (background, colour, border, font, radius, padding): `twaco mashup set <mashup> <widget> --style [part[@state]:]property=value`, repeatable. This writes the widget's entry in the root's StyleProperties, as Composer's style editor does. Use it for colour and custom CSS on one widget.
- A look copied from a design-kit widget of the same type: `--style-from <mashup>:<widget>`.
- A look shared by several widgets as a named style: the entity is a StyleDefinition, `twaco new style <name> --background ... --foreground ...`, changed with `twaco style set`.
- The whole page's theme (fonts, base colours of every PTC widget): a StyleTheme, `twaco new theme <name> [--from <StyleTheme>] [--tokens <preset|file>] [--prop name=value]`, chosen with `twaco mashup new --theme <name>` or `twaco mashup set <mashup> mashup-root --prop StyleTheme=<name>`.
- Free CSS for the page: `twaco mashup set <mashup> mashup-root --prop 'CustomMashupCss=.card { color: red; }'` (written to the managed `mashup/custom.css`). Prefer `--style`: it is validated and keyed by widget id.

## Examples

- A card frame: `twaco mashup set Acme.App.Orders_MU flexcontainer-4 --style background=#ffffff --style border-style=solid --style border-width=1px --style border-color=#BEC6CA --apply`
- A label's text colour and size: `--style label:color=#009bd6 --style label:font-size=24px`
- A hover colour on a collection cell: `--style cell@hover:background=#eee`
- A quoted value such as a font family: `--style "font-family='Arial Black'"`. Use single quotes inside the CSS value, so the argument has no double quotes to be split by the Windows shell.
- A grey page column or a white bordered card in one step: `twaco mashup recipe panel <mashup> <container> --style grey|card`.

## Rules

- `mashup set --style` names a widget; the root `mashup-root` takes `--prop CustomMashupCss=` or `--prop StyleTheme=`, so to colour the page style its page container (the first flexcontainer under the root) instead.
- An empty value removes a property.
- Say a value without `!important`, and do not place a widget with style (`position`, `top`, `transform`, `flex`): Composer places it from its own properties (`Width`, `flex-grow`, a wrapper row's `align-items` and `justify-content`). A container's `--style flex...` is refused.
- Run `twaco mashup validate <mashup>` after styling; it reports what the runtime is likely to ignore (`!important`, placement properties, parts a widget lacks). A chart's text and axes are not styled through parts: use theme variables through `new theme --tokens` (brutalist sets light chart text). Set per-field series colours with `mashup state-format set`.
- One status card (caption, value, status chip in one bordered box): `twaco mashup recipe card <mashup> <container> --title <caption> --say <value> --status <text> --kind ok`.

## Tokens and themes

Design tokens resolve locally. `twaco mashup tokens` lists `convergence` (default), `clean`
and `brutalist`; `twaco mashup tokens brutalist --json` shows resolved names and native theme
props. Presets contain twaco's own values. Convergence retains the existing colour names and
`mono`/`sans` values.

A spec's top-level `tokens: brutalist` or `tokens: ./tokens.yaml` overrides `[mashup] tokens`
in twaco.toml, then convergence. Spec paths are relative to the spec; config paths are relative
to twaco.toml. Command styling uses the config setting or convergence.

```yaml
version: 1
name: Acme
extends: brutalist
colors:
  accent: '#52A4E8'
roles:
  primary: accent
fonts:
  sans: Arial, sans-serif
radius:
  default: none
shadow:
  hard: 4px 4px 0 ink
vars:
  e-chart-tick-label-color: '${colors.paper}'
```

Token files are strict version 1 YAML. Sections are `colors`, `roles`, `fonts`, `radius`,
`border`, `shadow`, `space`, `chart` (a colour list), `vars`, and optional `block` defaults
(`border`, `shadow`, `radius`). Colour scales such as `primary-50` and `grey-900` are optional.
Roles (`primary`, `success`, `warning`, `danger`, `info`, `background`, `surface`,
`surface-alt`, `text`, `text-muted`, `heading`, `line`, `on-primary`, `on-dark`, `selected`) name
colours or hex values; `border` takes `width` and `color` for the `border:` style shorthand and
`line` for the native line thickness of inputs and grids. Qualified references use `${colors.ink}`, `${roles.text}`,
`${fonts.mono}`, etc. Unknown references and circular inheritance are refused with known names.
Inheritance accepts presets or another file relative to the extending file.

```sh
twaco new theme Acme.Page_TH --tokens brutalist
twaco new theme Acme.Page_TH --from Acme.Base_TH --tokens ./tokens.yaml --prop global-color-core-primary=#CCFF00 --apply
```

Themes overlay base props, token props, then `--prop`. The mapping sets native ptcs globals
(colours, text colours, fonts, radii), the primary button text and chart text and series; other
widget variables follow those globals through the platform's theme engine. Layout containers
stay borderless and lines keep their native thickness unless `vars` sets them, and a token set
to `inherit` writes no variable, so the platform value stays. `vars` overrides the mapping. Variable names must use lowercase hyphens:
`backgroundColor` is refused with a `global-color-bg-primary` hint. Well-formed names outside
twaco's mapping are allowed and reported in the plan.

`page.theme` names an existing StyleTheme. Tokens resolve style values and do not create a theme.
With explicit tokens and the default theme, the build plan prints a `new theme --tokens` reminder.
Generate and select the theme to restyle widgets natively, including chart axis and legend text.
Custom CSS is separate from tokens; use native theme variables for ptcs styling.

Colour properties take colour or role names, `font-family` takes font names, `border-radius`
takes radius names, and `box-shadow` takes shadow names. For example:

```yaml
tokens: brutalist
# inside page.slots:
# - block: {id: oee, color: pink, caption: OEE, value: 84, size: xl,
#           style: {border: ink, shadow: hard, radius: none}}
```

Style maps accept `background`, `color`, `border`, `font`, `shadow`, `radius`. A border colour
uses the token border width and a solid line. Brutalist blocks default to ink borders and hard
shadows; `size: xl` sets the value to 48px. A label with `band: true` grows to the full row width.
Derived specs retain explicit `tokens:` and keep derived hex colours as hex.

Command parity: set `[mashup] tokens = "brutalist"` in twaco.toml, then use
`twaco mashup set Acme.Page_MU <widget> --style box-shadow=hard --apply`. MCP provides
`mashup_tokens` and `entity_new` with `kind: "theme", tokens: "brutalist"`.
