# Commands

Every twaco command and flag, from `twaco` run with no arguments. Commands that change a server
print a plan unless given `--apply`; `call` is the exception, because twaco cannot tell whether a
service writes. A server command takes `--profile <name>` (default: `default`); see
[Configuration](CONFIGURATION.md#server-profiles). `twaco <command> --help` shows one command.

## Set up

Describe a solution, check the environment, serve agents, update twaco.

```text
init [--write]              propose a twaco.toml from the repository's own entities; --write
                            also writes AGENTS.md and CLAUDE.md where absent, and adds the
                            lines twaco needs to .gitignore
init --agents               write only those, where absent
doctor [--profile <name>]   what resolved, what is reachable, what is missing
projects                    the solution's projects and their deploy order
mcp [--tools <groups>]      serve the tools over MCP on stdio (TWACO_ROOT: the solution)
    --tools <groups>        only these tool groups, comma-separated (TWACO_MCP_TOOLS): core,
                            entity, refactor, mashup, permissions, data, server, reference;
                            default all
update [--apply]            compare with the latest release; --apply verifies its signature
                            and replaces this binary (TWACO_NO_UPDATE_CHECK=1: no daily notice)
completions <shell>         a completion script on stdout for bash, elvish, fish, powershell
                            or zsh: every command, flag and action, with descriptions
```

## Work on the repository

Offline: sidecars, gates, types, the service catalog, what a change reaches, a designer's export, renames.

```text
extract <entity>|--all      entity XML -> sidecars
sync <entity>|--all         sidecars -> entity XML
    --check                 report what would change, write nothing
    --allow-add-remove      add and remove services and fields as the sidecars do
    --relayout              rewrite scripts in the configured CDATA layout
fmt [--check]               format service scripts
check [--detail]            every gate, one exit code
    --live                  also parse every script on the server (fails closed)
types [--check [--json]|--platform] [--profile <name>]
                            generate declarations; --check runs TypeScript once
                            suppress one finding on the line above with
                            // @ts-ignore or // @ts-expect-error
                            [[check]] hook: command = ["twaco", "types", "--check", "--json"]
catalog [<entity>] [--project P] [--search <text>] [--json]
                            offline services, signatures, origins and descriptions
impact <entity> [--member <name>] [--min-confidence structural|resolved|review] [--depth <n>]
                            what changing an entity, or one service/property/field of it, reaches
    --detail                list every dependent and the chain of references to each
    --json                  the report as JSON (chains with --detail)
    --dot                   the dependents as a Graphviz graph
trace <Entity>.<Service> [--param <name>] [--json]
                            how each caller uses a service: what runs each mashup call, what sets
                            each parameter, where each result field goes, the arguments each script
                            passes and what it reads of the result, and what the service calls
    --param <name>          only that parameter's sources
unused [--min-confidence structural|resolved|review] [--collection <name>] [--fields]
                            entities no entry point reaches; advisory, deletes nothing
    --fields                result fields mashups bind, which no mashup binds or names
    --detail                list every one, with the entry points and each file
    --json                  the report as JSON
docs [--out <file>] [--force | --check]
                            the solution written down: projects and deploy order, inheritance,
                            services, DataShapes, and the references to review; Markdown
    --detail                every signature and field, and every review reference
    --json                  JSON instead of Markdown
    --out <file>            write it to one file (atomically) instead of printing; --force replaces
    --check                 with --out: write nothing, exit 1 when the file is missing or stale
adopt <export.xml>          take a designer's or backend collaborator's export; plan unless --apply
    --base <file|handoff|git rev>  collaborator's starting point; --only ui|backend
    --take theirs:<name>,ours:<name>  resolve conflicts (theirs:ui takes every UI one)
    --entity <name>; --detail; --json; --fail-on-revert
handoff list|record <file.xml>... --name <name> [--apply]
                            list recorded collaborator bases, or plan/copy one under .twaco/handoffs
rename entity <old> <new> [--apply] [--text] [--detail] [--json] [--skip-checks]
                            rename exactly one entity; plan unless --apply
    --sql | --sql-dir <dir> | --no-sql   the database half, as for rename field
rename prefix <old> <new> [--apply] [--text] [--detail] [--json] [--skip-checks]
                            rename a project/building-block prefix; --text includes other files
    --sql | --sql-dir <dir> | --no-sql   the database half, as for rename field
rename field <datashape> <old> <new> [--apply] [--detail] [--json] [--skip-checks]
                            rename a DataShape field and its configuration tables
    --text                  refused: a field rename has no text pass
    --sql | --sql-dir <dir> | --no-sql
                            entity, prefix and field: a rename that touches DBConnection tables is
                            refused until you choose: --sql writes the migration script (default
                            folder sql/, run it before the import), --no-sql says the tables are unused
rename service <entity> <old> <new> [--apply] [--detail] [--json] [--skip-checks]
                            rename a declared service, its overrides and repository callers
    --text                  refused: a service rename has no outside-text pass
rename param <entity> <service> <old> <new> [--apply] [--detail] [--json] [--skip-checks]
                            rename one declared service input and repository callers
    --text                  refused: a parameter rename has no outside-text pass
rename table <entity> <old> <new> [--apply] [--detail] [--json] [--skip-checks]
                            rename a declared configuration table and inherited instances
    --text                  refused: a table rename has no outside-text pass
rename property <entity> <old> <new> [--apply] [--detail] [--json] [--skip-checks]
                            rename a property of a Thing, template or shape, its overrides and readers
    --text                  refused: a property rename has no text pass
move service <from> <to> <name> [--as <new>] [--leave-delegate] [--apply] [--detail] [--json]
                            lift a service out of one Thing, template or shape and put it on another
    --as <new>              give it a new name on the target
    --leave-delegate        keep the service on the source, calling the moved one (a Thing target)
move property <from> <to> <name> [--as <new>] [--apply] [--detail] [--json]
                            the same for a property definition
copy service <from> <to> <name> [--as <new>] [--apply] [--detail] [--json]
                            as move, and the source keeps its service
copy property <from> <to> <name> [--as <new>] [--apply] [--detail] [--json]
retemplate <entity> [--to <template>] [--add-shapes a,b] [--remove-shapes a,b] [--accept-loss] [--apply] [--detail] [--json]
                            change a Thing's template (or a template's base) or its implemented shapes;
                            the plan lists what it and everything inheriting it gains and loses
    --to <template>         the new thingTemplate / baseThingTemplate
    --add-shapes a,b        implement these shapes too
    --remove-shapes a,b     stop implementing these shapes
    --accept-loss           go ahead although stored values or references would lose their definition
reproject <entity>... --to <project> [--apply] [--json]
                            move entities to another project: their projectName, and their files
                            under that project's root; the plan names each reference the move puts
                            outside the deploy order, with the depends_on that allows it
    --to <project>          the project they move to
widgets [<widget>] [--search <text>] [--json]
                            the embedded ThingWorx 10.2 widget catalogue: types, Composer labels,
                            aliases, containers and bindable properties, events and services
mashup validate <mashup>|--all [--json]
                            what in a mashup ThingWorx would load but not honour, or the next edit would
                            trip over: structure, ids, bindings and events that name what is not there,
                            catalogue contracts, several property maps, contained-mashup drift;
                            exit 1 when it finds any
mashup describe <mashup> [--json]
                            what a mashup holds and how it is wired, without its XML: the widget tree,
                            each service it calls with what sets each parameter (or that nothing does),
                            each binding and each event, in the addresses mashup bind and on take
mashup add <mashup> <widget> [--in <container> | --right-of <widget> | --left-of <widget> | --above <widget> | --below <widget>]
    [--label <text>] [--prop <name=value>]... [--input <name:TYPE>]... [--apply] [--json]
                            add a catalogue widget to a mashup; plan unless --apply; a function widget
                            (expression, validator, navigation...) goes beside the layout
    --in <container>        add inside a responsive container (center when omitted)
    --right-of/--left-of/--above/--below <widget>
                            place beside an existing widget
    --label <text>          set the widget's LabelText, Label, Text or Title property
    --prop <name=value>     set one simple catalogue default property, repeatable
    --input <name:TYPE>     an expression's or validator's input, repeatable
mashup add-contained <parent> <child> [--in <container> | --right-of <widget> | --left-of <widget> | --above <widget> | --below <widget>]
    [--set <parameter=value>]... [--apply] [--json]
                            add a child mashup as a contained mashup; plan unless --apply
    --in <container>        add inside a responsive container (center when omitted)
    --right-of/--left-of/--above/--below <widget>
                            place beside an existing widget
    --set <parameter=value> set a visible input parameter on the child
mashup bind <mashup> --from <address> --to <address> [--apply] [--json]
                            add one data binding to a mashup; plan unless --apply
    --from <address>        source address from mashup describe
    --to <address>          target address from mashup describe
mashup on <mashup> --event <address> --invoke <address> [--apply] [--json]
                            run a service or widget service when a mashup event fires; plan unless --apply
    --event <address>       event address from mashup describe
    --invoke <address>      data call or widget service to run
mashup param <add|set|remove> <mashup> <name> [--type <BASETYPE>] [--direction <IN|OUT|BOTH>]
    [--description <text>] [--default <value> | --no-default] [--data-shape <DataShape>] [--force] [--apply] [--json]
                            change a mashup parameter and every contained-mashup cache; plan unless --apply
    --type <BASETYPE>       required by add; ThingWorx base type
    --direction              IN, OUT or BOTH (BOTH by default for add)
    --default                a scalar default value
    --no-default             clear the default on set
    --data-shape             required for INFOTABLE
    --force                  remove bindings when removing a parameter
mashup recipe <recipe> <mashup> [<widget>] [recipe flags] [--apply] [--json]
                            write the wiring real mashups repeat, in one edit; plan unless --apply
    enable-when|show-when <widget> --if <condition> [--input name=<address>]...
                            an expression into the widget's Disabled or Visible
    text <label> --expr <expression> [--input name=<address>]...
                            a label's text computed from its inputs
    message --after <data call> (--say <text> | --from <address>) [--kind info|warning|error]
                            a status message when the call completes
    confirm <button> --then <handler> [--title <text>] [--say <text>]
                            a confirmation before the click runs the handler
    popup <widget>|--on <event> --mashup <name> [--window modal-popup|popup|replace|new]
        [--size <WxH>] [--title <text>] [--pass name=<address>]... [--reload <handler>]
                            open a mashup, pass it values, run the handler when it closes
    close-after --after <data call>
                            the popup closes itself when the call completes
    auto-refresh --then <data call> (--every <seconds> | --interval <address>)
                            run the call again every so often
    validate <widget>|--on <event> --if <condition> [--input name=<address>]... --then <handler>
        [--else-say <text>]   run the handler when the condition holds, else show the text
    chart <chart> --data <data call> --x <field> [--value <field>] [--kind <chart type>]
                            draw a service result: the category field (a pie: label and value)
    columns <grid> --column field[:Title[:width]]... [--data <data call>]
                            a grid's columns, rendered by each field's base type
    picker <dropdown|list> --data <data call> --x <display field> [--value <field>]
        [--selected <address>]   fill a dropdown or list from a service result
mashup event add <mashup> <name> --when <address> [--apply] [--json]
                            give a mashup an event its parents react to with mashup on --event
                            ui:<container>.<name>: a NUMBER parameter set to the time when the
                            event at --when fires; plan unless --apply
    --when <address>        the event in this mashup that fires it, as mashup on --event takes it
mashup templates [<name>] [--json]
                            list the embedded responsive mashup templates and their slots
mashup new <name> [--project <name>] [--template <name>] [--primary-menu <address>]
    [--secondary-menu <address>] [--theme <StyleTheme>] [--master]
    [--visibility <principal>]... [--param <name:TYPE[:DIRECTION]>]... [--tab <Label=Mashup>]...
    [--apply] [--json]
                            create a ThingWorx 10.2 responsive mashup as Composer would: its entity file
                            and its content sidecars; plan unless --apply
    --template              layout template name or alias (default: blank); see mashup templates
    --layout                compatibility alias for --template
    --primary-menu          required INFOTABLE service result for a template's menu bar
    --secondary-menu        optional INFOTABLE service result for a template's menu bar
    --theme <StyleTheme>    a StyleTheme in the solution (default: PTC Convergence Theme)
    --master                a master mashup, with a page container filling the center
    --visibility            Organization/<name> or OrganizationalUnit/<organization>:<unit>, repeatable;
                            none unless given
    --tab                   with --template tabs: a tab's label and the mashup it shows, repeatable
new building-block <name> [--type standard|abstract|implementation] [--display-name <text>] [--description <text>]
    [--parent <block>] [--model-logic] [--no-management-shape] [--root <dir>] [--base-extension PTC.Base:<version>]
    [--apply] [--json]
                            create a building block: its project, entry point, manager, groups and
                            organization as files, and its project in twaco.toml; plan unless --apply
    --type                  standard (own manager, default), abstract (no manager Thing) or implementation
    --parent <block>        the abstract block an implementation implements
    --model-logic           also a ModelLogic_TS shape
    --no-management-shape   an implementation without its own Management_TS
    --root <dir>            the project's folder (default: the block's name)
    --base-extension        the PTC.Base extension version to depend on (default: what another project declares)
thing property add <Entity> <name>:<TYPE> [--default <value>] [--persistent] [--logged]
    [--read-only] [--category <text>] [--units <text>] [--data-shape <DataShape>]
    [--description <text>] [--apply] [--json]
thing property set <Entity> <name>[:<TYPE>] [the add flags] [--no-default] [--no-persistent]
    [--no-logged] [--no-read-only] [--force] [--apply] [--json]
thing property remove <Entity> <name> [--force] [--apply] [--json]
                            add, change or remove a property of a Thing, ThingTemplate or
                            ThingShape; a persistent one also writes the stored value of every
                            Thing that has it; plan unless --apply
    --force                 remove or retype a property something still uses (they are listed)
thing subscription add <Entity> <name> --event <Event> [--source <Thing>]
    [--source-property <property>] (--then <code> | --script <file.js>) [--apply] [--json]
thing subscription remove <Entity> <name> [--apply] [--json]
                            add or remove a script subscription; no --source is the entity
                            itself; plan unless --apply
thing shape <add|remove> <Thing|ThingTemplate> <ThingShape> [--force] [--apply] [--json]
                            implement a ThingShape, or stop; refused when a member clashes, or
                            when something still uses a member the entity loses; plan unless --apply
    --force                 stop implementing it anyway (what uses its members is listed)
timer set <Timer> [--every <duration>] [--run-as <user>] [--enabled|--disabled] [--apply]
    [--json]
                            change a Timer Thing's settings; plan unless --apply
scheduler set <Scheduler> [--cron <quartz cron>] [--run-as <user>] [--enabled|--disabled]
    [--apply] [--json]
                            change a Scheduler Thing's settings; plan unless --apply
style set <StyleDefinition> [--background <color>] [--foreground <color>]
    [--secondary-background <color>] [--line-color <color>] [--line-style <style>]
    [--line-thickness <n>] [--bold|--no-bold] [--italic|--no-italic] [--underline|--no-underline]
    [--text-size <size>] [--image <MediaEntity>] [--display-string <text>] [--apply] [--json]
                            change only the given keys of a style; an empty color clears it;
                            plan unless --apply
media set <MediaEntity> --file <path> [--apply] [--json]
                            replace a MediaEntity's image (PNG, JPEG, GIF or SVG, up to 10 MB);
                            plan unless --apply
menu item add <Menu> <Title>=<Mashup> [--default] [--before <Title>] [--apply] [--json]
menu item remove <Menu> <Title> [--apply] [--json]
                            add or remove a mashup item, keeping the effective menu in step;
                            plan unless --apply
state set <StateDefinition> --type numeric|string --state <spec>... [--apply] [--json]
                            replace a StateDefinition's states (the specs of new state);
                            plan unless --apply
new thing <name> [--project <name>] [--description <text>] [--template <ThingTemplate>] [--shape <ThingShape>]...
    [--value-stream <ValueStream>] [--visibility <principal>]... [--apply] [--json]
                            create a ThingWorx Thing as a repository file; plan unless --apply
new thing-template <name> [--project <name>] [--description <text>] [--base <ThingTemplate>] [--shape <ThingShape>]...
    [--visibility <principal>]... [--apply] [--json]
                            create a ThingWorx ThingTemplate as a repository file; plan unless --apply
new thing-shape <name> [--project <name>] [--description <text>] [--visibility <principal>]... [--apply] [--json]
                            create a ThingWorx ThingShape as a repository file; plan unless --apply
new data-shape <name> --field <name:TYPE[:primary]>... [--project <name>] [--description <text>] [--base <DataShape>]
    [--visibility <principal>]... [--apply] [--json]
                            create a DataShape and its fields.json sidecar; plan unless --apply
new timer <name> --run-as <user> [--project <name>] [--every <duration>] [--disabled] [--then <code>|--script <file.js>]
    [--description <text>] [--visibility <principal>]... [--apply] [--json]
                            create a Timer Thing and optional Timer subscription; plan unless --apply
new scheduler <name> --run-as <user> [--project <name>] [--cron <quartz cron>] [--disabled] [--then <code>|--script <file.js>]
    [--description <text>] [--visibility <principal>]... [--apply] [--json]
                            create a Scheduler Thing and optional ScheduledEvent subscription; plan unless --apply
new data-table <name> --data-shape <DataShape> [--project <name>] [--description <text>]
    [--index name=field1,field2]... [--apply] [--json]
                            create a persistent DataTable and datatable.json; plan unless --apply
new stream <name> --data-shape <DataShape> [--project <name>] [--description <text>] [--apply] [--json]
                            create a persistent Stream; plan unless --apply
new value-stream <name> [--project <name>] [--description <text>] [--apply] [--json]
                            create a persistent ValueStream; plan unless --apply
new media <name> --file <path> [--project <name>] [--description <text>] [--apply] [--json]
                            create a PNG, JPEG, GIF, or SVG MediaEntity; plan unless --apply
new style <name> [--project <name>] [--description <text>] [--background <color>] [--foreground <color>]
    [--secondary-background <color>] [--line-color <color>] [--line-style solid|dashed|dotted|none]
    [--line-thickness <n>] [--bold] [--italic] [--underline] [--text-size <size>]
    [--image <MediaEntity>] [--display-string <text>] [--apply] [--json]
                            create a StyleDefinition; plan unless --apply
new state <name> --type numeric|string --state <spec>... [--project <name>] [--description <text>]
    [--apply] [--json]
                            create a StateDefinition; plan unless --apply
new menu <name> --item <Title=Mashup>... [--project <name>] [--description <text>] [--default]
    [--label <text>] [--apply] [--json]
                            create a Menu; plan unless --apply
new theme <name> [--project <name>] [--description <text>] [--from <StyleTheme>]
    [--prop name=value]... [--apply] [--json]
                            create a StyleTheme; plan unless --apply
```

## Deploy and compare

Against a server: what differs, what would be imported, and doing it.

```text
entity status [<entity>|--all] [--detail] [--record]
    --record                 record matching working/server hashes as baseline
entity get <entity>         fetch raw server XML to stdout or --out <path>
                            Collection/Name: any server entity; a bare name: the repository's
    [--profile <name>]      server profile (default: default)
entity push <entity>        import one entity, refusing if the server changed
    --apply                 actually push (without it, report what would happen)
    --force                 push over a server-side change; the server's copy is saved first
    --no-backup             with --force, do not save the server's copy first
entity delete <entity>...   plan guarded server deletion; Collection/Name or a bare server name
    --renamed               also delete undeleted old entity/prefix names from .twaco/renames.json
    --allow-repository-defined  accept deletion of an entity the repository still defines
    --allow-outside-dependents  accept structural dependents outside this delete set
    --allow-file-repository-data-loss  accept deletion of a FileRepository Thing and its files
    --force                 deprecated: means the first two acknowledgements, never FileRepository data loss
    --apply                 delete, confirm each entity is absent, and mark ledger entries
    --no-backup             do not save the server's copies under .twaco/backups first
    --json                  entities include refusal messages and parallel refusal_codes when refused
entity carry <old> <new>... copy run-time, design-time and visibility permissions of renamed entities
                            (pairs written Collection/Old Collection/New); principals follow the ledger
    --renamed               also every entity the rename ledger has not yet carried or deleted
    --apply                 write the differing permissions, read each back, mark the ledger
    --detail                also ask the platform for its own difference count
    --json                  {plan|applied, entities:[collection, old, new, status, kinds, error]}
entity restore [<set> [<entity>...]]  list backup sets, or plan importing one back
    --apply                 import the set's entities, confirming each on the server
    --json                  {sets|plan|applied, ...}
permissions init [--project <name>] [--from-helper] [--apply] [--json]
                            draft a permissions.toml for each project without one, from what it
                            grants today, so that `permissions apply` then changes nothing but
                            what its notes name; roles from the permission helper when there is
                            one; prints unless --apply
    --from-helper           take the grants from the helper's tables, not the entity XML (a
                            matrix edited in the helper's mashup)
permissions audit [--project <name>] [--server] [--detail] [--json]
                            each project's permissions.toml against its entity XML, offline: blocks
                            the policy would change, services a strict entity leaves unclassified,
                            principals the server refuses or no entity defines, rules that match
                            nothing; exit 1 on any error
    --server                also the server (read-only): each entity's permissions, the helper's
                            tables, the policy's platform grants and memberships, each role's unit
    --detail                every grant behind a finding
permissions apply [--project <name>] [--apply] [--detail] [--json]
                            write each project's permissions.toml into its entity XML: the run-time
                            block of each Thing, the instance run-time block of each shape and
                            template, and the role principals of each visibility block; only blocks
                            that differ change; plan unless --apply; refused while a strict
                            entity has an unclassified service
    --detail                every grant each file gains or loses
permissions diff <entity>...|--all [--json]
                            each entity's run-time, design-time and visibility permissions (and a
                            shape's or template's instance permissions) against the server's:
                            grants only the server has (an import never removes one), grants only
                            the repository has, and allow/deny that differs (an import keeps the
                            server's); exit 1 when any differs
permissions push <entity>...|--all|--platform [--apply] [--json]
                            make the server's permission sets exactly the repository's, set by set,
                            reading each back; plan unless --apply; records the baseline of pushed
                            entities that then match the server
    --platform              instead, add the policies' [[platform]] grants and memberships the
                            server lacks (what DeployComponent does); never removes anything
datatable copy <old> <new> [--map a=b,c=d] [--drop-unmapped] [--append] [--max-rows <n>] [--apply] [--json]
                            copy a DataTable's rows into the one that replaced it; plan unless --apply
    --map a=b,...           source field -> target field (same names and the rename ledger match otherwise)
    --drop-unmapped         leave behind source fields that have no target field
    --append                allow a target that already has rows
bundle [--backend-only] [--handoff <name>] [--apply]  one importable document
    --check                 report whether the bundle is current, write nothing
deploy [--apply] [--force]  check, bundle, live-parse, conflict-check, import
    --backend-only          exclude configured UI collections
    --only <entity>         include only this entity (repeatable)
    --only-projects <a,b>   include only these projects
    --skip-checks           skip offline gates; live parse still runs
    --no-backup             with --force, do not save the server's copies first
config-table <thing> <table> one Thing's configuration table on the server
    --backup <file>         save it (never overwrites a file)
    --restore <file>        put a backup back; a plan unless --apply
    --diff                  compare with the entity XML in the repository
    --detail                every row, not only the first
db run <file.sql> [--thing <name>] [--no-transaction] [--timeout <seconds>] [--apply] [--json]
                            run one atomic SQLCommand; plan unless --apply
db query <file.sql>|-q <sql> [--thing <name>] [--max-rows <n>] [--timeout <seconds>] [--detail] [--json]
                            run a read-only SQLQuery; summary shows columns and first 20 rows
db clean [--apply] [--json] delete temporary ZZ.Twaco.Sql.* Things an interrupted db run left; plan unless --apply
```

## Run and observe

Call services and read what the server says.

```text
call <target> <service> [<json>] [--timeout <seconds>] [--detail] [--as <test user>]
                            target: Collection/Name, or an entity by name or last segment
    --with-logs             then what it wrote to ScriptLog and ApplicationLog (waits up to 3 s)
    --as <test user>        call as a [[test.user]] with the profile's test_password (the profile
                            needs allow_test_users): 404 if the user cannot see it, 401 a bad password
logs <log>                  read a server log: ApplicationLog, ScriptLog, CommunicationLog,
                            ConfigurationLog, SecurityLog (newest first; times are local)
    --since <n><s|m|h|d>    how far back (default 1h); or --from <time> --to <time>
    --level <L>             L and above: TRACE, DEBUG, INFO, WARN, ERROR
    --grep <text>           entries whose message contains the text
    --regex <re>            a Java regex that must match the whole message
    --user/--thread/--origin <v>  exact matches
    --limit <n>             at most n entries (default 100); --oldest-first; --json
logs level <log> [<LEVEL>]  read a log's level and its subloggers, or plan setting it
    --sublogger <name>      a class or package within the log
    --reset                 a sublogger (or, without --sublogger, all) back to the log's level
    --apply                 make the change; it is the whole server's, and the undo is printed
settings                    the server's subsystems: running, and how many settings tables
settings <Subsystem> [<Table>]  every setting of one, with its value and description
settings --search <text>    settings whose name or description contains the text
                            (read-only; PASSWORD values are never shown); --json
```

## Server content

File repositories, extension packages, Composer-style exports and imports.

```text
repo list                   the server's file repositories
repo ls <repo> [<path>]     a folder's folders and files; --recursive for all below it; --json
repo get <repo> <path>      a file to stdout, or --out <file> (never overwritten without --force)
repo status <repo>          filerepository/<repo>/ against the server: same, differs,
                            local-only, remote-only (sizes, then SHA-256); --json
repo put <repo> <file> <path>  upload a file; --overwrite to replace one
repo mkdir <repo> <path>    create a folder
repo rm <repo> <path>       delete a file, or an empty folder; --recursive for one with content
repo mv <repo> <from> <to>  move a file; --overwrite to replace one
repo push <repo>            upload what filerepository/<repo>/ has that the server lacks
repo pull <repo>            download what the server has into filerepository/<repo>/
                            (neither deletes; a file differing on both sides needs --overwrite)
    --apply                 make the change: put, mkdir, rm and mv are plans without it
localization status         the solution's localization tables (localization/) against the
                            server: same, differs, local only, server only; problems; --json; --detail
localization pull           write the server's tokens of this solution into the table files
localization push           import the table files that differ, Default first, and read back
                            (--prune also deletes server tokens no file has, on pull removes
                            rows the server lacks)
localization new <table>    a table file for a language, seeded with the Default tokens;
                            --language-common, --language-native, --description, --project
localization set <token> --value <text>  add or change a token; --table (default Default),
                            --usage (new: label), --context, --project
localization remove <token> remove a token from every table, or from --table
    --table <name>          one table only; --apply makes the change (all but status plan)
users list [--json]         the test users of [[test.user]] on the server: present, signing in
                            with the profile's test_password, in their groups
users seed                  create the missing users, the [test] project and group memberships
users delete                delete the test users; both need allow_test_users = true in the
                            profile and plan until --apply
test [<filter>...]          the service cases of tests/*.toml: each called as each of its test
                            users, expecting a status and optionally rows; checks and lists them
                            until --apply runs them (a case may change the server; the profile
                            must say allow_test_users = true); exit 1 when one fails
    --detail                every passing call too
    --json                  the calls run as JSON
ext list [--json]           the server's extension packages
ext show <package>          one package: its extensions, and which are in use
ext import <zip>            validate a package on the server; --apply installs it
ext remove <package>        plan removing a package (refused while in use); --apply removes it
search [<text>] [--type <T>[,<T>]] [--project P] [--limit <n>] [--json]
                            the server's entity search (Composer's Spotlight): text
                            in a name or description, or a pattern with *
export entity <Coll/Name> --out <file>   one entity's XML, from the server's Exporter
export collection <Coll> [--project P] --out <file>   a collection, or one project's part of it
export project <P> --out <file>  everything of a project, as one XML
    --force                 replace the --out file if it exists
export source-control --repository R --path <p> [--project P] [--collection C] [--tags t]
    [--zip <name>] [--with-dependents]  the source-control layout into a repository; --apply sends it
import <file.xml|.zip>      import an export into the server: a plan of what it adds and
                            replaces unless --apply
import source-control --repository R --path <p>  a source-control tree from a repository;
                            the plan is the server's diff; --apply imports, then diffs again
    --detail                every entity of the plan, not only the counts
    --overwrite-properties --overwrite-tables  replace the server's property values and
                            configuration table rows (kept by default, as in Composer)
```

## Release

Package the repository, offline.

```text
package bundle [--project P] [--backend-only|--frontend-only] --out <file>
                            the repository as one importable XML, offline
package source-control [--project P] --out <file.zip>  the <Project>/<Collection>/<Name>.xml layout
package extension [--project P] [--editable] --out <file.zip>  an extension package: one project's,
                            or the solution's as a zip of its projects' ([package] in twaco.toml)
    --force                 replace the --out file if it exists
```

## Knowledge

Workflow, platform quirks, project documents, the help center and the Java API.

```text
guide                       the knowledge topics: twaco's workflow, the platform's verified quirks,
                            the service-code reference, and the solution's own markdown
guide <topic> [--section <heading>]  read one (a long one gives its outline)
guide --search <words>      the sections that best match; --limit <n>; --json
help search <words>         search the ThingWorx Platform help (the server's version)
help page <page>            read a help page as Markdown; --section <heading>
    --version <10.1>        another release; --limit <n>; --refresh; --json
    --profile <name>        the server whose version to read (default: default)
javadoc search <name>       search ThingWorx Platform Java API classes and members
    --limit <n>; --refresh; --json
javadoc class <Name|pkg.Name>  read a class as Markdown; --member <name>; --refresh; --json
```

## Shared

Flags described once for the commands that list them, and exit codes.

```text
--project <name>            narrow to one project of the solution
--profile <name>            the server profile (default: default)
--log <filter>              diagnostic logs on stderr, for any command (or TWACO_LOG);
                            a level such as debug, or a directive such as twaco::core::server=trace
--log-file <path>           append diagnostic logs to a file instead (or TWACO_LOG_FILE)
--version                   twaco's version and the commit it was built from
<command> --help            one command, and its flags

exit: 0 done, 1 a --check found work, 2 failed
```
