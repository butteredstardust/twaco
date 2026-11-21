# Task index

I want to ... -> command (use --help for flags; most writers plan until --apply)

- Create an entity of any kind -> `twaco new --help`
- Create a mashup -> `twaco mashup new <name>`
- Choose a mashup layout -> `twaco mashup templates`
- Find a widget -> `twaco widgets --search <words>`
- Add a widget -> `twaco mashup add <mashup> <widget>`
- Lay out widgets -> `twaco mashup recipe --help`
- Bind widget data -> `twaco mashup bind --help`
- Add a service -> `twaco thing service add <Entity> <Name>`
- Replace a service body, signature kept -> `twaco thing service set <Entity> <Name> --script <file.js>`
- Add a property -> `twaco thing property add <Entity> <name>:<TYPE>`
- Add a subscription -> `twaco thing subscription --help`
- Create a DataShape with fields -> `twaco new data-shape <name> --field <name:TYPE>`
- Edit DataShape fields -> edit its fields.json, then `twaco sync <DataShape> --check --allow-add-remove`
- Deploy changes -> `twaco deploy --help`
- Call a service -> `twaco call <Entity>.<Service>` (runs immediately)
- Read server logs -> `twaco logs`
- Compare with the server -> `twaco entity status <entity>`
- Manage permissions -> `twaco permissions --help`
- Colour, font or custom CSS on a widget -> `twaco mashup set <mashup> <widget> --style color=#fff` (guide: styling)
- Theme the whole mashup -> `twaco new theme <name> --from <StyleTheme>`; apply it with `mashup new --theme` (guide: styling)
