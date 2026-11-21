# Styling a mashup

Colour, font, spacing and custom CSS for a mashup: which twaco command changes which layer.

## Style, colour, custom CSS or theme: which layer

- A widget's own look (background, colour, border, font, radius, padding): `twaco mashup set <mashup> <widget> --style [part[@state]:]property=value`, repeatable. This writes the widget's entry in the root's StyleProperties, as Composer's style editor does. Use it for colour and custom CSS on one widget.
- A look copied from a design-kit widget of the same type: `--style-from <mashup>:<widget>`.
- A look shared by several widgets as a named style: the entity is a StyleDefinition, `twaco new style <name> --background ... --foreground ...`, changed with `twaco style set`.
- The whole page's theme (fonts, base colours of every PTC widget): a StyleTheme, `twaco new theme <name> [--from <StyleTheme>] [--prop name=value]`, chosen with `twaco mashup new --theme <name>` or `twaco mashup set <mashup> mashup-root --prop StyleTheme=<name>`.
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
- Run `twaco mashup validate <mashup>` after styling; it reports what the runtime is likely to ignore (`!important`, placement properties, parts a widget lacks). A chart's text, axes and bars are not styled through parts: set series colours with `mashup state-format set`.
- One status card (caption, value, status chip in one bordered box): `twaco mashup recipe card <mashup> <container> --title <caption> --say <value> --status <text> --kind ok`.
